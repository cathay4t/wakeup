# wakeup

A small Linux daemon that sends desktop notifications when long-running work
finishes, so you can switch away from the terminal without missing the result.

It watches two things:

- **Shell tasks** — `make`, `tar`, `wget`, and `xz` processes discovered via
  `/proc`. A notification is sent when one exits, provided it ran for at least
  60 seconds.
- **opencode sessions** — top-level [opencode](https://opencode.ai) sessions
  that have been waiting for input for more than 60 seconds while an opencode
  instance is still running in the session's directory. If the session's
  opencode window is focused, no notification is sent. Sending new input
  cancels the pending notification.

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

The process polls every 30 seconds and logs to stderr. It does not fork into
the background, so run it under your init system or a terminal multiplexer.

## License

Apache 2.0
