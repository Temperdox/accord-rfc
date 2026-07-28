# Ringtones

Drop audio files here and they appear in **Settings → Voice → Incoming call
sound** the next time the client is built. No code or config change is needed:
`build.rs` scans this folder and compiles whatever it finds into the binary, so
the ringtones work identically from `cargo run` and from an installed app.

- **Formats**: `.mp3`, `.wav`, `.flac`, `.ogg`/`.oga`, `.m4a`/`.mp4` (decoded by
  symphonia, pure Rust — no system codecs involved).
- **Naming**: the filename is the label. `soft-chime.mp3` shows as "Soft chime";
  hyphens and underscores become spaces.
- **Stability**: the filename (without extension) is also the saved id, so
  renaming a file resets anyone who had chosen it back to the default.
- **Length**: keep them a few seconds. Playback loops until the call is answered
  or dismissed, so a short phrase with its own trailing silence sounds best.
- **Size**: everything here is embedded in the executable, so prefer a modest
  bitrate — a few hundred KB each rather than a full-length track.

The built-in "Classic" option is synthesised in code (`voice_engine/ringtone.rs`)
and is always available, including when this folder is empty.

Only add files you have the right to distribute — they ship inside the app.
