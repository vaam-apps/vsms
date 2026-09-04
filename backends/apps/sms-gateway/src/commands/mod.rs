#![doc = include_str!("commands.md")]

pub(crate) mod bootstrap;
pub(crate) mod common;
pub(crate) mod create_app;
pub(crate) mod healthcheck;
pub(crate) mod provision_client;
pub(crate) mod provision_user;
pub(crate) mod record_route_validation;
pub(crate) mod rotate_signing_key;
pub(crate) mod routes;
pub(crate) mod seed_console_client;
pub(crate) mod seed_dispatch;
pub(crate) mod serve;
