use std::io::Write;
use std::path::Path;
use virtual_terminal::{VtEvent, VtRecording};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: vt_replay <file.vtr>");
    let recording = VtRecording::from_file(Path::new(&path)).unwrap();

    let mut output_count = 0u32;
    let mut input_count = 0u32;
    let mut resize_count = 0u32;
    let mut output_bytes = 0usize;
    let mut last_ts = 0u32;

    for event in &recording.events {
        match event {
            VtEvent::Output {
                timestamp_us,
                data,
            } => {
                output_count += 1;
                output_bytes += data.len();
                last_ts = *timestamp_us;
            }
            VtEvent::Input { timestamp_us, .. } => {
                input_count += 1;
                last_ts = *timestamp_us;
            }
            VtEvent::Resize {
                timestamp_us,
                rows,
                cols,
            } => {
                resize_count += 1;
                last_ts = *timestamp_us;
                eprintln!(
                    "  resize @ {:.3}s -> {}x{}",
                    *timestamp_us as f64 / 1e6,
                    cols,
                    rows
                );
            }
        }
    }

    let duration_s = last_ts as f64 / 1e6;
    eprintln!();
    eprintln!(
        "Header: {}x{}, scrollback={}",
        recording.header.cols, recording.header.rows, recording.header.scrollback
    );
    eprintln!(
        "Events: {} output ({} bytes), {} input, {} resize",
        output_count, output_bytes, input_count, resize_count
    );
    eprintln!("Duration: {:.3}s", duration_s);

    // Replay into a VT and emit scrollback + visible screen as ANSI
    let mut vt = recording.replay(64 * 1024);
    let state = vt.debug_state();

    eprintln!();
    eprintln!(
        "Final screen: {}x{}, cursor=({},{}), alt={}, scrollback={}",
        state.screen_size.1,
        state.screen_size.0,
        state.cursor_position.1,
        state.cursor_position.0,
        state.alternate_screen,
        state.scrollback_depth
    );
    eprintln!();

    // Emit the full replay (scrollback + visible screen) as ANSI to stdout
    let replay = vt.replay(state.screen_size.0);
    std::io::stdout().write_all(&replay).unwrap();
    std::io::stdout().flush().unwrap();
}
