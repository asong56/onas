mod cli;
mod image;
mod audio;
mod video;
mod meta;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn")
    ).init();

    match Cli::parse().command {
        Command::Image(a) => image::run(a),
        Command::Audio(a) => audio::run(a),
        Command::Video(a) => video::run(a),
        Command::Meta(a)  => meta::run(a),
    }
}
