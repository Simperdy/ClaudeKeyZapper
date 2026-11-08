//! KeyZapper core: profiles, key file, Claude Code settings binding, gateway checks, backups and IT policies.
//! Shared by the desktop app (Tauri) and `keyzapper-helper`; behaviour matches the macOS app 1.x.

pub mod i18n;

pub mod agents;
pub mod audit;
pub mod backup;
pub mod backup_store;
pub mod claude_desktop;
pub mod claude_office;
pub mod cli;
pub mod clipboard;
pub mod errors;
pub mod gateway;
pub mod hermes;
pub mod git;
pub mod helper;
pub mod json;
pub mod keys;
pub mod legacy_keychain;
pub mod managed;
pub mod mcp;
pub mod mcp_catalog;
pub mod metadata;
pub mod models;
pub mod oidc;
pub mod paths;
pub mod proxy;
pub mod settings;
pub mod subscription;
pub mod time;
pub mod tokens;
pub mod tools;
pub mod update;
pub mod whats_new;
