use crab_city_ui::Assets;
use rust_embed::Embed;

fn main() {
    // v2
    println!("Testing embedded assets...\n");

    // List all embedded files
    println!("Embedded files:");
    for file in Assets::iter() {
        println!("  - {}", file);
    }
    println!();
}
