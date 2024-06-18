use std::path::PathBuf;

use clap::Parser;
use once_cell::sync::Lazy;

#[derive(Debug, Parser)]
pub struct Args {
    #[arg(long)]
    pub(crate) directory: Option<PathBuf>,
}

pub static ARGUMENTS: Lazy<Args> = Lazy::new(Args::parse);
