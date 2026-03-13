//! VT session capture/replay for golden tests.
//!
//! Records PTY output, input, and resize events with microsecond timestamps
//! into a CBOR stream. Events are flushed to disk as they happen so
//! recordings survive crashes and kills.
//!
//! # Wire format
//!
//! The file is a sequence of self-delimiting CBOR values:
//!
//! 1. One `VtRecordingHeader` (initial terminal dimensions + scrollback config)
//! 2. Zero or more `VtEvent` values (output/input/resize with timestamps)
//!
//! Reading stops at EOF. A partial trailing CBOR value (from a crash) is
//! silently ignored — all previously flushed events are still recoverable.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::VirtualTerminal;

/// Recording header — written once at the start of the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VtRecordingHeader {
    pub rows: u16,
    pub cols: u16,
    pub scrollback: u32,
}

/// A single recorded event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VtEvent {
    /// PTY output bytes.
    Output { timestamp_us: u32, data: Vec<u8> },
    /// Input sent to PTY.
    Input { timestamp_us: u32, data: Vec<u8> },
    /// Terminal resize.
    Resize {
        timestamp_us: u32,
        rows: u16,
        cols: u16,
    },
}

/// Records VT session events, streaming each event to the underlying writer.
///
/// Generic over `W: Write` — use `File` for production (crash-safe) or
/// `Vec<u8>` for tests. The header is written on construction; each event
/// is flushed immediately.
pub struct VtRecorder<W: Write> {
    writer: io::BufWriter<W>,
    start: Instant,
}

impl VtRecorder<std::fs::File> {
    /// Open a recording file, write the header, and return the recorder.
    pub fn open(path: &Path, rows: u16, cols: u16, scrollback: u32) -> io::Result<Self> {
        let f = std::fs::File::create(path)?;
        Self::new(f, rows, cols, scrollback)
    }
}

impl<W: Write> VtRecorder<W> {
    /// Create a recorder that streams to `writer`. Writes the header immediately.
    pub fn new(writer: W, rows: u16, cols: u16, scrollback: u32) -> io::Result<Self> {
        let mut w = io::BufWriter::new(writer);
        let header = VtRecordingHeader {
            rows,
            cols,
            scrollback,
        };
        ciborium::into_writer(&header, &mut w).map_err(cbor_to_io)?;
        w.flush()?;
        Ok(Self {
            writer: w,
            start: Instant::now(),
        })
    }

    /// Record PTY output bytes.
    pub fn output(&mut self, data: &[u8]) {
        let event = VtEvent::Output {
            timestamp_us: self.elapsed_us(),
            data: data.to_vec(),
        };
        let _ = self.write_event(&event);
    }

    /// Record input bytes sent to the PTY.
    pub fn input(&mut self, data: &[u8]) {
        let event = VtEvent::Input {
            timestamp_us: self.elapsed_us(),
            data: data.to_vec(),
        };
        let _ = self.write_event(&event);
    }

    /// Record a resize event.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let event = VtEvent::Resize {
            timestamp_us: self.elapsed_us(),
            rows,
            cols,
        };
        let _ = self.write_event(&event);
    }

    /// Consume the recorder and return the underlying writer (flushed).
    pub fn into_inner(mut self) -> io::Result<W> {
        self.writer.flush()?;
        self.writer
            .into_inner()
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn elapsed_us(&self) -> u32 {
        self.start.elapsed().as_micros() as u32
    }

    fn write_event(&mut self, event: &VtEvent) -> io::Result<()> {
        ciborium::into_writer(event, &mut self.writer).map_err(cbor_to_io)?;
        self.writer.flush()
    }
}

/// A parsed recording: header + events.
#[derive(Debug, Clone)]
pub struct VtRecording {
    pub header: VtRecordingHeader,
    pub events: Vec<VtEvent>,
}

impl VtRecording {
    /// Parse a recording from a reader. Reads the header, then events until EOF.
    /// A partial trailing value (from a crash) is silently ignored.
    pub fn parse(r: impl Read) -> io::Result<Self> {
        let mut r = io::BufReader::new(r);

        let header: VtRecordingHeader = ciborium::from_reader(&mut r).map_err(cbor_de_to_io)?;

        let mut events = Vec::new();
        loop {
            match ciborium::from_reader::<VtEvent, _>(&mut r) {
                Ok(event) => events.push(event),
                Err(ciborium::de::Error::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                // Silently stop on any other deserialization error at the
                // stream boundary — likely a partial write from a crash.
                Err(_) => break,
            }
        }

        Ok(VtRecording { header, events })
    }

    /// Parse a recording from a file path.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let f = std::fs::File::open(path)?;
        Self::parse(f)
    }

    /// Replay the recording into a new VirtualTerminal, returning the final state.
    ///
    /// `max_delta_bytes` controls the VT's auto-compaction threshold.
    pub fn replay(&self, max_delta_bytes: usize) -> VirtualTerminal {
        let mut vt = VirtualTerminal::new(
            self.header.rows,
            self.header.cols,
            max_delta_bytes,
            self.header.scrollback as usize,
        );

        for event in &self.events {
            match event {
                VtEvent::Output { data, .. } => {
                    vt.process_output(data);
                }
                VtEvent::Input { .. } => {
                    // Input events are logged for completeness; skip on VT replay
                }
                VtEvent::Resize { rows, cols, .. } => {
                    vt.resize(*rows, *cols);
                }
            }
        }

        vt
    }
}

fn cbor_to_io<T: std::fmt::Debug>(e: ciborium::ser::Error<T>) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

fn cbor_de_to_io<T: std::fmt::Debug>(e: ciborium::de::Error<T>) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create an in-memory recorder, return bytes after consuming it.
    fn record_to_bytes(f: impl FnOnce(&mut VtRecorder<Vec<u8>>)) -> Vec<u8> {
        let mut rec = VtRecorder::new(Vec::new(), 24, 80, 10_000).unwrap();
        f(&mut rec);
        rec.into_inner().unwrap()
    }

    #[test]
    fn roundtrip_empty() {
        let buf = record_to_bytes(|_| {});
        let parsed = VtRecording::parse(&buf[..]).unwrap();
        assert_eq!(parsed.header.rows, 24);
        assert_eq!(parsed.header.cols, 80);
        assert_eq!(parsed.header.scrollback, 10_000);
        assert!(parsed.events.is_empty());
    }

    #[test]
    fn roundtrip_events() {
        let buf = record_to_bytes(|rec| {
            rec.output(b"Hello, world!\r\n");
            rec.input(b"\x03"); // Ctrl-C
            rec.resize(40, 120);
            rec.output(b"After resize\r\n");
        });

        let parsed = VtRecording::parse(&buf[..]).unwrap();
        assert_eq!(parsed.events.len(), 4);

        assert!(
            matches!(&parsed.events[0], VtEvent::Output { data, .. } if data == b"Hello, world!\r\n")
        );
        assert!(matches!(&parsed.events[1], VtEvent::Input { data, .. } if data == b"\x03"));
        assert!(matches!(
            &parsed.events[2],
            VtEvent::Resize {
                rows: 40,
                cols: 120,
                ..
            }
        ));
        assert!(
            matches!(&parsed.events[3], VtEvent::Output { data, .. } if data == b"After resize\r\n")
        );
    }

    #[test]
    fn replay_produces_correct_screen() {
        let buf = record_to_bytes(|rec| {
            rec.output(b"Line 1\r\nLine 2\r\nLine 3");
        });

        let parsed = VtRecording::parse(&buf[..]).unwrap();
        let vt = parsed.replay(4096);

        assert_eq!(vt.cursor_position(), (2, 6));

        let screen = vt.screen();
        let text: String = (0..80)
            .map(|c| {
                screen.cell(0, c).map_or(" ".to_string(), |cell| {
                    let s = cell.contents();
                    if s.is_empty() {
                        " ".to_string()
                    } else {
                        s.to_string()
                    }
                })
            })
            .collect::<String>();
        assert!(text.starts_with("Line 1"));
    }

    #[test]
    fn replay_applies_resize() {
        let buf = record_to_bytes(|rec| {
            rec.output(b"Before resize");
            rec.resize(40, 120);
            rec.output(b"\r\nAfter resize");
        });

        let parsed = VtRecording::parse(&buf[..]).unwrap();
        let vt = parsed.replay(4096);

        let screen = vt.screen();
        assert_eq!(screen.size(), (40, 120));
    }

    #[test]
    fn timestamps_increase() {
        let buf = record_to_bytes(|rec| {
            rec.output(b"first");
            rec.output(b"second");
        });

        let parsed = VtRecording::parse(&buf[..]).unwrap();
        let t0 = match &parsed.events[0] {
            VtEvent::Output { timestamp_us, .. } => *timestamp_us,
            _ => panic!(),
        };
        let t1 = match &parsed.events[1] {
            VtEvent::Output { timestamp_us, .. } => *timestamp_us,
            _ => panic!(),
        };
        assert!(t1 >= t0);
    }

    #[test]
    fn truncated_event_survives() {
        let mut buf = record_to_bytes(|rec| {
            rec.output(b"good event");
        });

        // Append garbage — simulates a partial write from a crash
        buf.extend_from_slice(&[0xBF, 0x63, 0x66]); // partial CBOR map

        let parsed = VtRecording::parse(&buf[..]).unwrap();
        // The good event should survive; the partial trailing data is ignored
        assert_eq!(parsed.events.len(), 1);
        assert!(matches!(&parsed.events[0], VtEvent::Output { data, .. } if data == b"good event"));
    }

    #[test]
    fn file_roundtrip() {
        let dir = std::env::temp_dir().join("vt_recorder_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.vtr");

        {
            let mut rec = VtRecorder::open(&path, 24, 80, 5000).unwrap();
            rec.output(b"hello\r\n");
            rec.resize(40, 120);
            rec.input(b"x");
            // recorder dropped here — flushes on drop
        }

        let parsed = VtRecording::from_file(&path).unwrap();
        assert_eq!(parsed.header.rows, 24);
        assert_eq!(parsed.header.scrollback, 5000);
        assert_eq!(parsed.events.len(), 3);
        assert!(matches!(&parsed.events[0], VtEvent::Output { data, .. } if data == b"hello\r\n"));
        assert!(matches!(
            &parsed.events[1],
            VtEvent::Resize {
                rows: 40,
                cols: 120,
                ..
            }
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
