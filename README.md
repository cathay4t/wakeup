# wakeup

A small Linux daemon that sends desktop notifications when long-running work
finishes, so you can switch away from the terminal without missing the result.

It watches two things:

- **Shell tasks** — `make`, `tar`, `wget`, and `xz` processes discovered via
  `/proc`. A notification is sent when one exits, provided it ran for at least
  60 seconds.
- **opencode sessions** — top-level [opencode](https://opencode.ai) sessions
  that have finished and are waiting for input while an opencode instance is
  still running in the session's directory. A notification is sent immediately
  when the result arrives, unless the session's opencode window is focused. If
  the window was focused when the result arrived, no notification is sent for
  that turn.

Notifications are delivered through `notify-send`, with the application name
`wakeup`.

## Requirements

- Linux (uses `/proc`)
- `notify-send` from libnotify
- Rust 1.99 or newer (edition 2024)
- `opencode` on `PATH` for session monitoring (optional)
- `swaymsg` on `PATH` to suppress notifications for focused opencode windows
  (optional, Sway only)

## Build

```sh
cargo build --release
```

The binary is written to `target/release/wakeup`.

## Usage

```sh
./target/release/wakeup
```

The process checks opencode every 5 seconds and shell tasks every 30 seconds,
and logs to stderr. It does not fork into the background, so run it under your
init system or a terminal multiplexer.

## License

Apache 2.0
