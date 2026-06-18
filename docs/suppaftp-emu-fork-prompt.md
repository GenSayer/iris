# Task prompt: fork suppaftp into a transport-generic FTP client (`suppaftp-emu`)

> Hand this to an implementation agent (or use as a spec). It is self-contained.
> Upstream: <https://github.com/veeso/suppaftp> (MIT). Our consumer: the IRIS SGI
> Indy emulator, which needs an FTP client that talks to a guest OS over an
> **emulated/virtual network**, not host TCP sockets.

## Goal

Produce a **synchronous FTP client crate generic over the transport stream**,
derived from suppaftp's protocol code, so an embedder can supply its own
`Read + Write` connection (e.g. a virtual-network stream inside an emulator)
instead of `std::net::TcpStream`. Default behaviour with a built-in TCP
connector must be byte-for-byte equivalent to today's suppaftp. Keep the change
shaped so it could become an upstream "bring-your-own-transport" PR.

**Why:** suppaftp is hardcoded to `std::net::TcpStream` (control channel, data
channel, and the `TlsStream` trait all demand a concrete `TcpStream`). An
emulator's guest is not reachable via a host socket — it lives on a virtual
Ethernet the emulator relays in-process. The client must open both its control
and (passive) data connections through an embedder-provided connector.

## Scope

- **Sync client only** for v1 (`sync_ftp`). Leave `async_ftp` as-is or
  feature-gate it off; do not block on async.
- **Passive mode is mandatory; active mode optional** (default to an
  "active unsupported" error for custom transports — emulator uses passive).
- **TLS is out of scope on the generic path.** Keep the existing TLS support for
  the default `TcpStream` connector (feature-gated, unchanged), but the
  bring-your-own-transport path is plaintext (the virtual net is already
  isolated). Do **not** try to layer TLS over an arbitrary transport in v1.
- Keep all protocol logic intact: `command.rs`, `status.rs`, `list.rs` (+
  `list/*`), `regex.rs`, `types.rs`, `command/feat.rs`.

## Target design

Introduce a transport abstraction and make the client generic over it.

```rust
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::time::Duration;

/// Opens FTP connections for a client. The default impl uses std::net::TcpStream;
/// an embedder (e.g. an emulator) supplies its own to reach a guest over a
/// virtual network. `Stream` is the control/data connection type.
pub trait FtpConnector {
    type Stream: Read + Write + std::fmt::Debug;

    /// Open the control connection, or a passive-mode data connection, to `addr`.
    /// In passive mode `addr` is the address parsed from the server's `227`
    /// reply; a virtual-network connector may route it to the guest directly.
    fn connect(&self, addr: SocketAddr) -> FtpResult<Self::Stream>;

    fn connect_timeout(&self, addr: SocketAddr, _t: Duration) -> FtpResult<Self::Stream> {
        self.connect(addr)
    }

    // Active mode is optional. Default: unsupported so passive-only transports
    // (the emulator) compile without implementing it. The built-in TcpConnector
    // overrides these to preserve suppaftp's current active-mode behaviour.
    type Listener;
    fn bind(&self) -> FtpResult<(Self::Listener, SocketAddr)> {
        Err(FtpError::ActiveModeUnsupported) // add this variant
    }
    fn accept(&self, _l: Self::Listener) -> FtpResult<Self::Stream> {
        Err(FtpError::ActiveModeUnsupported)
    }
}

/// Default connector: plain std::net::TcpStream — preserves existing behaviour.
pub struct TcpConnector;
impl FtpConnector for TcpConnector {
    type Stream = std::net::TcpStream;
    type Listener = std::net::TcpListener;
    fn connect(&self, addr: SocketAddr) -> FtpResult<TcpStream> { /* TcpStream::connect */ }
    fn connect_timeout(&self, addr, t) -> FtpResult<TcpStream> { /* TcpStream::connect_timeout */ }
    fn bind(&self) -> FtpResult<(TcpListener, SocketAddr)> { /* bind 0.0.0.0:0 + local_addr */ }
    fn accept(&self, l) -> FtpResult<TcpStream> { /* l.accept().0 */ }
}
```

Make the client generic over the connector + its stream:

```rust
pub struct FtpStream<C: FtpConnector = TcpConnector> {
    connector: C,
    reader: BufReader<C::Stream>,   // control connection
    // ...existing fields (mode, welcome msg, features, ...) minus TcpStream-isms
}
```

The data stream becomes generic over the connector's stream (drop the
TLS-coupled enum on the generic path):

```rust
// Plain transport-generic data stream (no TLS variant here).
pub struct DataStream<S: Read + Write>(pub S);
impl<S: Read + Write> Read  for DataStream<S> { /* delegate */ }
impl<S: Read + Write> Write for DataStream<S> { /* delegate */ }
```

Keep the existing TLS `DataStream`/`TlsStream` machinery only on the
`TcpConnector` path behind the TLS feature, so default users are unaffected.

## Exact edits (sync path)

`crates/suppaftp/src/sync_ftp/mod.rs`:
- Make `ImplFtpStream` generic over `C: FtpConnector`; `reader:
  BufReader<C::Stream>`.
- `connect<A: ToSocketAddrs>(addr)` and `connect_timeout(SocketAddr, Duration)`:
  keep as convenience constructors **on `FtpStream<TcpConnector>`** (resolve
  `ToSocketAddrs`, call `TcpConnector.connect`). Add a general
  `with_connector(connector: C, addr: SocketAddr) -> FtpResult<Self>` and
  `connect_with_stream(stream: C::Stream)` (generic).
- Replace the `PassiveStreamBuilder` field with the connector. `data_command()`:
  passive → `self.connector.connect(addr)`; active → `self.connector.bind()` +
  `accept()`. Wrap the result in the new generic `DataStream`.
- `active()`: route through `self.connector.bind()` instead of
  `TcpListener::bind("0.0.0.0:0")`.
- `pasv()` / `parse_passive_address_from_response()` stay (they already return a
  `SocketAddr`; the connector decides how to reach it).

`crates/suppaftp/src/sync_ftp/data_stream.rs`:
- Add the generic `DataStream<S>` above; leave the TLS enum for the TCP path.

`crates/suppaftp/src/types.rs`:
- Add `FtpError::ActiveModeUnsupported`.

## Crate packaging

- New crate `suppaftp-emu` (working name) under a fork of veeso/suppaftp, or a
  thin standalone crate that vendors the sync protocol modules. Preserve MIT
  license + attribution to veeso/suppaftp in headers and README.
- Public surface the embedder needs: `FtpStream::with_connector`, the
  `FtpConnector` trait, `TcpConnector`, the generic `DataStream`, and the
  re-exported command/list/status/types modules.
- IRIS depends on it by `path`/`git` initially.

## Tests / acceptance

1. **All existing suppaftp sync tests pass** with the default `TcpConnector`
   (behaviour unchanged). Keep the `test_container` integration tests for the
   TCP path.
2. **New: in-memory mock transport.** Implement an `FtpConnector` over a pair of
   in-process bidirectional pipes (e.g. `std::io::Cursor` scripts or a
   `os_pipe`/`crossbeam` byte channel) backed by a tiny scripted FTP server, and
   drive `USER`, `PASS`, `TYPE I`, `PASV`, `LIST`, `RETR`, `STOR`, `CWD`, `PWD`,
   `MKD`, `DELE`. Assert the bytes round-trip without any real socket.
3. Compiles with `--no-default-features` (no TLS) and the BYO-transport path.
4. `cargo clippy` clean on the sync path.

## IRIS integration (consumer side — informational, not part of this crate)

IRIS will implement:
```rust
struct VirtualConnector { /* handle to the NatEngine in-process peer seam */ }
impl FtpConnector for VirtualConnector {
    type Stream = VirtualTcpStream; // Read+Write over the emulated NIC
    type Listener = ();
    fn connect(&self, addr: SocketAddr) -> FtpResult<VirtualTcpStream> {
        // open a TCP connection to `addr` on the guest's virtual network
        // (control = guest:21; passive data = the guest IP:port from `227`,
        // reachable directly because we're an endpoint on that net).
    }
}
```
No ALG/PASV-rewrite is needed here — the connector reaches the guest's advertised
passive address on the virtual network directly.
