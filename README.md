# tuimon

A terminal dashboard for live CPU, memory, network interfaces, disks, processes, open ports
and system messages, built with
[ratatui](https://ratatui.rs) and [sysinfo](https://crates.io/crates/sysinfo).

```
 tuimon  my-host │ up 2d 05h 57m │ load 1.36 1.35 1.06 │ every 1.00s
╭ CPU 2.3% · AMD Ryzen 5 PRO 4650U · 1461 MHz ─────────────────────────╮
│100%│                                  0   1.0% ━━─────────────────────│
│    │                                  1   4.0% ━━━────────────────────│
│0%  │                        ⠠⠤⠤⠤⣀⣀    …                              │
╰──────────────────────────────────────────────────────────────────────╯
╭ Memory · 14.8 GiB total ──────────╮╭ Network · all ──────────────────╮
│RAM   5.79 GiB / 14.8 GiB  39.0% ━━││▼ RX   6.62 KiB/s  total 5.96 GiB │
│Swap  1.02 GiB / 20.0 GiB   5.1% ━─││▲ TX   1.58 KiB/s  total 6.69 GiB │
╰───────────────────────────────────╯╰─────────────────────────────────╯
```

## Features

- **CPU** — total usage history chart, per-core usage bars (wrapping into
  columns on many-core machines), model name and average frequency.
- **Memory** — RAM and swap usage with a combined history chart.
- **Network** — RX/TX rates with an auto-scaling history chart. Cycle between
  the aggregate of all non-loopback interfaces and each individual interface.
- **Disk** — used space across all filesystems, total read/write rates and
  their history chart. On terminals narrower than 120 columns it gets its own
  row below memory and network.
- **Network tab** — every interface with its link state, RX/TX rates, totals
  and IPv4/IPv6 addresses, like `ip -brief address`. The selected interface
  gets a detail panel (MAC, MTU, packet rates, errors, every address) and its
  own rate history chart. Starts on the busiest non-loopback interface.
  Press `c` / `C` to copy its IPv4 / IPv6 address (routable addresses are
  preferred over link-local ones).
- **Disks tab** — every physical disk (NVMe, SSD, HDD, removable, including
  unmounted ones) with its model, size, read/write rates, busy % (like
  `iostat`'s `%util`) and mount points. Below it, every mounted filesystem
  like `df -hT` with a usage bar, and a read/write history chart for the
  selected disk. Filesystems on LUKS or LVM are traced back to the disk
  they live on. Linux only.
- **Processes tab** — every process (including kernel threads) with user,
  CPU%, memory, state and uptime, like `top`. Sort by CPU, memory, PID or
  name; filter with `/`. The selection sticks to the same PID as rows move.
- **Ports tab** — listening TCP/UDP sockets (or every socket, toggled with
  `a`) with owning user, PID and process, read directly from `/proc/net`.
  Linux only.
- **Messages tab** — the systemd journal streamed live (including kernel
  messages), colored by priority, with a detail pane for the full selected
  message. Filter by minimum level, kernel-only (like `dmesg`), or text.
  Follows new messages like `tail -f` until you scroll up.
- **Shell equivalents** — each panel shows the standard command that
  produces the same data (`mpstat`, `free`, `sar`, `ip`, `iostat`, `lsblk`,
  `df`, `top`, `ss`,
  `journalctl`), updated live as you change the interval, sort or filters.
- Header with hostname, uptime and load average.
- Pause, and adjust the refresh rate at runtime.

## Usage

```sh
cargo install tuimon
tuimon --tick-rate 500 --history 240
```

Or from a clone of this repository: `cargo run --release`.

tuimon targets Linux: the disks and ports tabs read `/sys` and `/proc`
directly, and the messages tab uses the systemd journal.

| Option              | Default | Description                                  |
| ------------------- | ------- | -------------------------------------------- |
| `-t, --tick-rate`   | `1000`  | Refresh interval in ms (clamped to 250–10000) |
| `-H, --history`     | `120`   | Number of samples kept for charts (≥ 10)     |

### Keys

| Key                   | Action                              |
| --------------------- | ----------------------------------- |
| `q` / `Esc` / `^C`    | Quit (`Esc` first clears a filter)  |
| `p` / `Space`         | Pause / resume                      |
| `+` / `-`             | Slower / faster refresh             |
| `Tab` / `Shift-Tab`   | Next / previous tab                 |
| `1`–`6`               | Jump to tab                         |
| `↑`/`↓`, `j`/`k`      | Move selection (also `^N`/`^P`)     |
| `^D` / `^U`           | Half a page down / up               |
| `^F` / `^B`, `PgDn`/`PgUp` | Full page down / up            |
| `gg` / `G`, `Home`/`End` | Jump to first / last row         |

Per tab:

| Tab        | Keys                                                                 |
| ---------- | -------------------------------------------------------------------- |
| Overview   | `n` / `N` next / previous network interface                          |
| Network    | `c` copy IPv4 address · `C` copy IPv6 address                        |
| Disks      | Selection picks the disk whose I/O is charted                        |
| Processes  | `s` cycle sort · `r` reverse · `/` filter                            |
| Ports      | `a` listening only ↔ all sockets                                     |
| Messages   | `l` cycle minimum level · `d` kernel only · `f` follow · `/` filter  |

While typing a filter, `Enter` applies it and `Esc` clears it.

## Project layout

| File              | Responsibility                                          |
| ----------------- | ------------------------------------------------------- |
| `src/main.rs`     | CLI parsing and terminal setup/teardown                 |
| `src/app.rs`      | Tabs, event loop and global keys                        |
| `src/view.rs`     | Per-tab list state: selection, sorting, filtering       |
| `src/metrics.rs`  | CPU/memory/network/disk collection and history buffers  |
| `src/disks.rs`    | Physical disks and I/O from `/sys/block`, filesystems   |
| `src/processes.rs`| Process listing and sorting via `sysinfo`               |
| `src/ports.rs`    | Socket listing from `/proc/net/*` and `/proc/*/fd`      |
| `src/journal.rs`  | Background `journalctl --follow --output=json` reader   |
| `src/ui/`         | Rendering: shared chrome in `mod.rs`, one file per tab  |
| `src/clipboard.rs`| Copying text via `wl-copy`/`xclip`/`xsel` or OSC 52     |
| `src/format.rs`   | Byte / rate / duration formatting                       |

The `mpstat`, `sar` and `iostat` commands shown in the UI come from the `sysstat`
package (`sudo apt install sysstat`); `free` and `ss` are installed by
default on most distributions.

Without root, the Ports tab can only show the process for sockets you own,
the same limitation as `ss -p`. Run `sudo $(which tuimon)` to see every
process (`sudo` alone won't find a binary in `~/.cargo/bin`).

Copying addresses uses `wl-copy` (Wayland) or `xclip` / `xsel` (X11), and
falls back to the OSC 52 escape sequence when none is available, e.g. over
SSH. Not every terminal supports OSC 52, and GNOME Terminal is one that doesn't.

The Messages tab needs `journalctl` and permission to read the system
journal (membership in the `systemd-journal` or `adm` group, or root).

## Development

```sh
cargo test
cargo clippy --all-targets   # pedantic lints are enabled in Cargo.toml
cargo fmt
```
