#![doc = include_str!("../docs/base.md")]

mod client;
pub mod config;
pub mod error;
pub mod operation;
mod ws; // 新增（crate 内部使用）

pub use client::{Client, ClientBuilder};
pub(crate) mod oss_util;
