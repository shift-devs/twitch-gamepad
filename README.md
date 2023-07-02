# twitch-gamepad

This is a virtual gamepad that takes input from twitch chat. Only works on Linux with uinput.

## Setup

### Configuration

At any parent directory including the directory of the executable, create a file called `twitch_gamepad.toml`
containing the following:

```toml
[twitch]
channel_name = "your_channel_here"
```

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

## Privileges

Below are user privileges, ordered from greatest to least. Each level is granted all privileges from levels below them.

| Privilege | Allowed Actions |
| - | - |
| Broadcaster | All below actions |
| Channel Moderator | All below actions, block/unblock, op/deop users |
| Operator | All below actions |
| Standard | Submit movement commands |
