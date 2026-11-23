//! The link to the SyntHub service: the account, the catalogue, the relay door and
//! the taking of sounds. One module until the repository `synthub` is born; the
//! bridge reads it only through the paths the frontier guard lists.

pub mod catalogue;
pub mod command;
pub mod connect;
pub mod learn;
pub mod mcp;
pub mod online;
pub mod relay_protocol;
pub mod take;
