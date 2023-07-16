# twitch-gamepad

This is a virtual gamepad that takes input from twitch chat. Only works on Linux with uinput.

## Setup

### Configuration

At any parent directory including the directory of the executable, create a file called `twitch_gamepad.toml`
containing the following:

```toml
[twitch]
channel_name = "your_channel_here"

[twitch.auth]
type = "Anonymous"

[games]
game_name = "game-command"
```

See `twitch_gamepad.toml.example` for a full example config.

### Building

Requires a recent version of Rust stable to build, and the uinput kernel module.

```sh
sudo modprobe uinput
sudo usermod -a -G input $(whoami)
# Log out and log back in to apply group changes
cargo run
```

Alternatively, path to `twitch_gamepad.toml` can be specified on the command line:

```sh
cargo r -- /path/to/config.toml
```

## Usage

Commands can be entered either through stdin or in the chat of the provided channel.

### Movement Commands

These commands will result in gamepad inputs to be made. Each movement command can include a second argument
containing the duration to hold the command in seconds (only integer values accepted). The default if no argument
is provided is half a second. Buttons can be held for a max of 5 seconds.

`a` presses the A button for half a second

`a 5` presses the A button for 5 seconds

Multiple commands can be issued in a single command to be executed simultaneously, e.g. `a b 5` or `lt rt start select`

Below is a table of all movement commands. Commands are case insensitive.

| Command | Result |
| - | - |
| `a` | A button |
| `b` | B button |
| `c` | C button |
| `x` | X button |
| `y` | Y button |
| `z` | Z button |
| `tl` | Left Trigger |
| `tr` | Right Trigger |
| `up` | Up D-Pad Button |
| `down` | Down D-Pad Button |
| `left` | Left D-Pad Button |
| `right` | Right D-Pad Button |
| `start` | Start |
| `select` | Select |

### Moderation Commands

The following commands are of the form `tp <command> <parameters...>` and facilitate moderation.

| Command | Result |
| - | - |
| `tp block <username> [duration]` | Blocks the user for the specified optional duration. Duration can be specified in the form `1d2h10m5s`. No duration implies an indefinite block. |
| `tp unblock <username>` | Unblocks a user |
| `tp op <username>` | Gives operator privilege to a user |
| `tp deop <username>` | Removes operator privilege from a user |
| `tp game <game>` | Switches to the selected game |
| `tp stop` | Stops the current game |
| `tp list games` or `tp games` | List available games |
| `tp list blocked` | List blocked users |
| `tp list ops` | List operators |
| `tp help` | List all commands |
| `tp save/load` | Save or load state
| `tp reset` | Reset game |
| `tp mode democracy/anarchy` | Set mode, anarchy removes all blocks and cooldowns |
| `tp cooldown <duration>` | Sets cooldown per message, does not apply to operators and above |

## Privileges

Below are user privileges, ordered from greatest to least. Each level is granted all privileges from levels below them.

| Privilege | Allowed Actions |
| - | - |
| Broadcaster | All below actions |
| Channel Moderator | All below actions, block/unblock, op/deop users, switch games, set mode and cooldown |
| Operator | All below actions, can bypass cooldowns, save/load and reset games |
| Standard | Submit movement commands |
