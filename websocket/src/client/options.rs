use super::error::Error;
use super::result::Result;
use super::Handshake;
use js_sys::Object;
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue};
use workflow_wasm::object::*;

#[derive(Default)]
pub struct Options {
    // placeholder for future settings
    // TODO review if it makes sense to impl `reconnect_interval`
    pub receiver_channel_cap: Option<usize>,
    pub sender_channel_cap: Option<usize>,
    pub handshake: Option<Arc<dyn Handshake>>,
}

/// `ConnectionStrategy` specifies how the WebSockeet `async fn connect()` 
/// function should behave during the first-time connectivity phase.
#[derive(Default, Clone)]
pub enum ConnectStrategy {
    /// Continiously attempt to connect to the server. This behavior will
    /// block `connect()` function until the connection is established.
    #[default]
    Retry,
    /// Causes `connect()` to return immediately if the first-time connection
    /// has failed.
    Fallback,
}

impl ConnectStrategy {
    pub fn new(retry: bool) -> Self {
        if retry {
            ConnectStrategy::Retry
        } else {
            ConnectStrategy::Fallback
        }
    }

    pub fn is_fallback(&self) -> bool {
        matches!(self, ConnectStrategy::Fallback)
    }
}

/// 
/// `ConnectOptions` is used to configure the `WebSocket` connectivity behavior.
/// 
#[derive(Clone)]
pub struct ConnectOptions {
    /// Indicates if the `async fn connect()` method should return immediately
    /// or block until the connection is established.
    pub block_async_connect: bool,
    /// [`ConnectStrategy`] used to configure the retry or fallback behavior.
    pub strategy: ConnectStrategy,
    /// Optional `url` that will change the current URL of the WebSocket.
    pub url: Option<String>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            block_async_connect: true,
            strategy: ConnectStrategy::Retry,
            url: None,
        }
    }
}

impl ConnectOptions {
    pub fn fallback() -> Self {
        Self {
            block_async_connect: true,
            strategy: ConnectStrategy::Fallback,
            url: None,
        }
    }
    pub fn reconnect_defaults() -> Self {
        Self {
            block_async_connect: true,
            strategy: ConnectStrategy::Retry,
            url: None,
        }
    }
}

impl TryFrom<JsValue> for ConnectOptions {
    type Error = Error;
    fn try_from(args: JsValue) -> Result<Self> {
        let options = if let Some(args) = args.dyn_ref::<Object>() {
            let url = args.get("url")?.as_string();
            let block_async_connect = args.get("block")?.as_bool().unwrap_or(true);
            let strategy = ConnectStrategy::new(args.get("retry")?.as_bool().unwrap_or(true));

            ConnectOptions {
                block_async_connect,
                strategy,
                url,
            }
        } else if let Some(retry) = args.as_bool() {
            ConnectOptions {
                block_async_connect: true,
                strategy: ConnectStrategy::new(retry),
                url: None,
            }
        } else {
            ConnectOptions::default()
        };

        Ok(options)
    }
}
