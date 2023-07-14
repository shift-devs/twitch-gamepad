use rusqlite::{params, types::FromSql, Connection, OptionalExtension, ToSql, Transaction};
use std::path::Path;

#[cfg(test)]
pub fn clear_db(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("delete from users", ())?;
    conn.execute("delete from operators", ())?;
    conn.execute("delete from blocked_users", ())?;
    conn.execute("delete from last_command_time", ())?;
    Ok(())
}

fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "create table if not exists users (
             id integer primary key,
             twitch_id text not null unique,
             name text not null unique
         )",
        (),
    )?;

    conn.execute(
        "create table if not exists operators (
             id integer primary key,
             twitch_id text not null unique references users(twitch_id)
         )",
        (),
    )?;

    conn.execute(
        "create table if not exists blocked_users (
             id integer primary key,
             twitch_id text not null unique references users(twitch_id),
             unblock_time text
         )",
        (),
    )?;

    conn.execute(
        "create table if not exists last_command_time (
             id integer primary key,
             twitch_id text not null unique references users(twitch_id),
             time text not null
         )",
        (),
    )?;

    conn.execute(
        "create table if not exists config_kv (
             id integer primary key,
             key text not null unique,
             value text
         )",
        (),
    )?;

    Ok(())
}

#[cfg(test)]
pub fn in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    init_db(&conn)?;
    Ok(conn)
}

pub fn connect<T: AsRef<Path>>(path: T) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    init_db(&conn)?;
    Ok(conn)
}

pub fn set_kv<T: ToSql>(conn: &Connection, key: &str, value: T) -> rusqlite::Result<()> {
    conn.execute(
        "insert or replace into config_kv (key, value) values (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn get_kv<T: FromSql>(conn: &Connection, key: &str) -> rusqlite::Result<Option<T>> {
    conn.query_row(
        "select value from config_kv where key=?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
}

pub fn get_or_set_kv<T: ToSql + FromSql>(
    conn: &mut Connection,
    key: &str,
    default: T,
) -> rusqlite::Result<T> {
    let tx = conn.transaction()?;
    let query_val: Option<T> = tx
        .query_row(
            "select value from config_kv where key=?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;

    match query_val {
        Some(val) => Ok(val),
        None => {
            tx.execute(
                "insert into config_kv (key, value) values (?1, ?2)",
                params![key, default],
            )?;
            tx.commit()?;
            Ok(default)
        }
    }
}

pub fn update_user(conn: &Connection, id: &str, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "insert or replace into users(twitch_id, name) values (?1, ?2)",
        params![id, name],
    )?;
    Ok(())
}

pub fn test_and_set_cooldown_lapsed(
    conn: &mut Connection,
    id: &str,
    cooldown: &chrono::Duration,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let last_command_time: Option<chrono::DateTime<chrono::Utc>> = tx
        .query_row(
            "select time from last_command_time where twitch_id=?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;

    let cooldown_lapsed = match last_command_time {
        Some(last_command_time) => chrono::Utc::now() >= last_command_time + *cooldown,
        None => true,
    };

    tx.execute(
        "insert or replace into last_command_time (twitch_id, time) values (?1, ?2)",
        params![id, chrono::Utc::now()],
    )?;
    tx.commit()?;

    Ok(cooldown_lapsed)
}

pub fn is_operator(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "select id from operators where twitch_id=?1",
        params![id],
        |row| row.get(0),
    )
    .optional()
    .map(|opt: Option<Option<u64>>| opt.flatten().is_some())
}

pub fn is_blocked(conn: &mut Connection, id: &str) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let row: Option<(u64, Option<chrono::DateTime<chrono::Utc>>)> = {
        let mut query =
            tx.prepare("select id, unblock_time from blocked_users where twitch_id=?1")?;
        let mut rows = query.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?;

        if let Some(x) = rows.next() {
            x.ok()
        } else {
            None
        }
    };

    if row.is_none() {
        return Ok(false);
    }
    let (id, unblock_time) = row.unwrap();

    if unblock_time.is_some_and(|time| time <= chrono::Utc::now()) {
        // Block duration has lapsed, unblock the user
        tx.execute("delete from blocked_users where id=?1", params![id])?;
        tx.commit()?;
        Ok(false)
    } else {
        Ok(true)
    }
}

fn get_user_id_from_name(tx: &mut Transaction, name: &str) -> rusqlite::Result<Option<String>> {
    tx.query_row(
        "select twitch_id from users where name=?1",
        params![name],
        |row| row.get(0),
    )
    .optional()
}

pub fn block_user(
    conn: &mut Connection,
    name: &str,
    until: Option<chrono::DateTime<chrono::Utc>>,
) -> rusqlite::Result<bool> {
    let mut tx = conn.transaction()?;
    match get_user_id_from_name(&mut tx, name) {
        Ok(Some(twitch_id)) => {
            tracing::info!("Found id {}, blocking", twitch_id);
            tx.execute(
                "insert or replace into blocked_users(twitch_id, unblock_time) values (?1, ?2)",
                params![twitch_id, until],
            )?;
            tx.commit()?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn unblock_user(conn: &mut Connection, name: &str) -> rusqlite::Result<()> {
    let mut tx = conn.transaction()?;
    match get_user_id_from_name(&mut tx, name) {
        Ok(Some(twitch_id)) => {
            tracing::info!("Found id {}, unblocking", twitch_id);
            tx.execute(
                "delete from blocked_users where twitch_id=?1",
                params![twitch_id],
            )?;
            tx.commit()?;
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn list_blocked_users(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "select u.name from users u inner join blocked_users b on b.twitch_id = u.twitch_id",
    )?;
    let users: rusqlite::Result<Vec<String>> = stmt.query_map((), |row| row.get(0))?.collect();
    users
}

pub fn op_user(conn: &mut Connection, name: &str) -> rusqlite::Result<bool> {
    let mut tx = conn.transaction()?;
    match get_user_id_from_name(&mut tx, name) {
        Ok(Some(twitch_id)) => {
            tracing::info!("Found id {}, opping", twitch_id);
            tx.execute(
                "insert or replace into operators(twitch_id) values (?1)",
                params![twitch_id],
            )?;
            tx.commit()?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn deop_user(conn: &mut Connection, name: &str) -> rusqlite::Result<()> {
    let mut tx = conn.transaction()?;
    match get_user_id_from_name(&mut tx, name) {
        Ok(Some(twitch_id)) => {
            tx.execute(
                "delete from operators where twitch_id=?1",
                params![twitch_id],
            )?;
            tx.commit()?;
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn list_op_users(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "select u.name from users u inner join operators o on o.twitch_id = u.twitch_id",
    )?;
    let users: rusqlite::Result<Vec<String>> = stmt.query_map((), |row| row.get(0))?.collect();
    users
}
