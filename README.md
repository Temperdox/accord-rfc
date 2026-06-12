# Accord

Accord is a chat platform I'm building in Rust. It has two kinds of conversations:

- Public chats: unencrypted, community-scale channels. The server can read and
  search these, and it stores full history.
- Private chats: end-to-end encrypted direct messages and small groups using MLS
  (RFC 9420). For these the server is just an opaque relay; it only ever sees
  ciphertext.

The whole stack is Rust. The server is a tonic gRPC service, and the desktop app
is built with Tauri. Importantly, the desktop app can also host its own server
in-process, so a normal user can run a server without Docker or any setup.

Status: alpha. It builds and the core flows work, but it is early and rough.

## What works today

- Register, log in (JWT access tokens + refresh tokens), and chat in public
  channels in real time over a gRPC stream.
- End-to-end encrypted 1:1 DMs via MLS (OpenMLS). The server relays opaque
  ciphertext and never holds keys.
- Self-contained hosting: the client embeds the server and runs it with SQLite
  and an in-process message bus, so no Postgres, Redis, or Docker is required.
- Create and join servers using opaque invite keys (one paste-able string that
  carries the address, the TLS cert to pin, mesh peers if any, and an invite
  token). No manual configuration.
- Roles and permissions modeled on Discord (a 64-bit permission bitfield,
  assignable roles, an `@everyone` base role, owner override). The API enforces
  permissions on every privileged action.
- TLS on the transport. Self-hosted servers use a self-signed certificate that I
  pin through the invite key, so the connection is authenticated and encrypted
  with no certificate authority.
- Optional Yggdrasil mesh transport (behind a build feature) for reaching servers
  across the internet without port forwarding. This is experimental.
- Verbose logging and a dev-only menu for testing.

## Repository layout

```
Accord/
  proto/                     protobuf contract (one source of truth for the API)
  crates/
    accord-types/            shared ids, invite keys, permission bitfield
    accord-proto/            generated gRPC code (protox + tonic, no protoc)
    accord-crypto/           client-side crypto (Ed25519, Argon2id + XChaCha20, BLAKE3)
    accord-mls/              client-side MLS engine (OpenMLS)
    accord-server/           gRPC server as a library + binary
    accord-client/           Tauri desktop app
      src/                     React + TypeScript frontend
      src-tauri/               Rust shell + gRPC client + embedded host + mesh
  migrations/                SQL migrations (postgres/ and sqlite/)
  .run/                      RustRover run configurations
  .github/workflows/         CI + client-installer release build
  docker-compose.yml         Postgres + Redis + MinIO (for the Postgres path)
```

The one rule I keep strict: `accord-server` never depends on `accord-crypto` or
`accord-mls`. The dependency graph itself guarantees the server cannot do MLS or
touch key material. See ARCHITECTURE.md.

## Quick start (development)

Prerequisites: Rust (stable, edition 2024), Node.js 18+ and npm, and (only for
the Postgres path) Docker.

Build the frontend once, then run the client:

```
cd crates/accord-client
npm install
npm run build
cd ../..
cargo run -p accord-client
```

From the landing screen you can:

- Create a server (private or public). The app hosts it and logs you in as the
  owner. For a private server, click "Invite people" to get an invite key to
  share.
- Join a server by pasting an invite key.
- Use the Advanced tab to connect to a server URL directly.

You can also run the server or client by pressing the green run icon in RustRover.

## Self-hosting and networking

- The self-contained server (SQLite + in-process bus) is the default for
  client-hosted servers, so there is nothing to install.
- For LAN testing see TESTING-LAN.md.
- For internet reach over the Yggdrasil mesh see TESTING-INTERNET.md (and
  TESTING-MESH-SPIKE.md for the lower-level daemon spike).
- The separate, standalone server I would run on real infrastructure for large
  public hosting is kept out of this repository for now. CI only builds the
  client.

## Common commands

```
cargo build --workspace            build everything
cargo test  --workspace            run all tests
cargo run -p accord-server         run the standalone server (Postgres or SQLite)
cargo run -p accord-client         run the desktop app
docker compose up -d               start Postgres + Redis + MinIO (Postgres path)
```

End-to-end checks you can run without a GUI (they each start an in-process server):

```
cargo run -p accord-server --example embedded_smoke        self-hosting round trip
cargo run -p accord-server --example private_invite_smoke  invite-gated registration
cargo run -p accord-server --example roles_smoke           roles and permissions
cargo run -p accord-server --example tls_smoke             TLS with cert pinning
```

## Roadmap

- Done: public chat, private MLS DMs, self-contained hosting, invite keys, roles
  and permissions, TLS with cert pinning.
- Experimental: Yggdrasil mesh transport.
- Next: key-based identity, persisting client MLS state across restarts, key
  backup wiring, larger private groups, and a roles UI in the client.

## Tech

gRPC with tonic, async with tokio, storage with sqlx (Postgres or SQLite), cache
and pub/sub with redis (optional), auth with jsonwebtoken and argon2, crypto with
ed25519-dalek, chacha20poly1305, and blake3, MLS with openmls, mesh with
yggdrasil-ng, and the desktop app with tauri plus React and Vite.
