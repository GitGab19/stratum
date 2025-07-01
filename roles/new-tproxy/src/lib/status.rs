//! ## Status Reporting System for Translator
//!
//! This module defines how internal components of the Translator report
//! health, errors, and shutdown conditions back to the main runtime loop in `lib/mod.rs`.
//!
//! At the core, tasks send a [`Status`] (wrapping a [`State`]) through a channel,
//! which is tagged with a [`Sender`] enum to indicate the origin of the message.
//!
//! This allows for centralized, consistent error handling across the application.

use crate::error::{self, TproxyError};

/// Identifies the component that originated a [`Status`] update.
///
/// Each sender is associated with a dedicated side of the status channel.
/// This lets the central loop distinguish between errors from different parts of the system.
#[derive(Debug)]
pub enum Sender {
    /// Sender for downstream connections.
    Downstream(async_channel::Sender<Status>),
    /// Sender for downstream listener.
    Sv1Server(async_channel::Sender<Status>),
    /// Sender for bridge connections.
    ChannelManager(async_channel::Sender<Status>),
    /// Sender for upstream connections.
    Upstream(async_channel::Sender<Status>),
}

impl Sender {
    /// Sends a status update.
    pub async fn send(&self, status: Status) -> Result<(), async_channel::SendError<Status>> {
        match self {
            Self::Downstream(inner) => inner.send(status).await,
            Self::Sv1Server(inner) => inner.send(status).await,
            Self::ChannelManager(inner) => inner.send(status).await,
            Self::Upstream(inner) => inner.send(status).await,
        }
    }
}

impl Clone for Sender {
    fn clone(&self) -> Self {
        match self {
            Self::Downstream(inner) => Self::Downstream(inner.clone()),
            Self::Sv1Server(inner) => Self::Sv1Server(inner.clone()),
            Self::ChannelManager(inner) => Self::ChannelManager(inner.clone()),
            Self::Upstream(inner) => Self::Upstream(inner.clone()),
        }
    }
}

/// The kind of event or status being reported by a task.
#[derive(Debug)]
pub enum State {
    /// Sv1Server connection shutdown.
    Sv1ServerShutdown(TproxyError),
    /// Upstream connection shutdown.
    UpstreamShutdown(TproxyError),
    /// Upstream connection trying to reconnect.
    ChannelManagerShutdown(TproxyError),
    /// Component is healthy.
    Healthy(String),
}

/// Wraps a status update, to be passed through a status channel.
#[derive(Debug)]
pub struct Status {
    pub state: State,
}

/// Sends a [`Status`] message tagged with its [`Sender`] to the central loop.
///
/// This is the core logic used to determine which status variant should be sent
/// based on the error type and sender context.
async fn send_status(
    sender: &Sender,
    e: TproxyError,
    outcome: error_handling::ErrorBranch,
) -> error_handling::ErrorBranch {
    match sender {
        Sender::Downstream(tx) => {
            tx.send(Status {
                state: State::Healthy(e.to_string()),
            })
            .await
            .unwrap_or(());
        }
        Sender::Sv1Server(tx) => {
            tx.send(Status {
                state: State::Sv1ServerShutdown(e),
            })
            .await
            .unwrap_or(());
        }
        Sender::ChannelManager(tx) => {
            tx.send(Status {
                state: State::ChannelManagerShutdown(e),
            })
            .await
            .unwrap_or(());
        }
        Sender::Upstream(tx) => {
            tx.send(Status {
                state: State::UpstreamShutdown(e),
            })
            .await
            .unwrap_or(());
        }
    }
    outcome
}

/// Centralized error dispatcher for the Translator.
///
/// Used by the `handle_result!` macro across the codebase.
/// Decides whether the task should `Continue` or `Break` based on the error type and source.
pub async fn handle_error(sender: &Sender, e: error::TproxyError) -> error_handling::ErrorBranch {
    tracing::error!("Error: {:?}", &e);
    match e {
        TproxyError::VecToSlice32(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::BadCliArgs => send_status(sender, e, error_handling::ErrorBranch::Break).await,
        TproxyError::BadSerdeJson(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::BadConfigDeserialize(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::BinarySv2(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::CodecNoise(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::FramingSv2(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::InvalidExtranonce(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::Io(_) => send_status(sender, e, error_handling::ErrorBranch::Break).await,
        TproxyError::ParseInt(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::UpstreamIncoming(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::SubprotocolMining(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::PoisonLock => send_status(sender, e, error_handling::ErrorBranch::Break).await,
        TproxyError::ChannelErrorReceiver(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::TokioChannelErrorRecv(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::SetDifficultyToMessage(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::TargetError(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Continue).await
        }
        TproxyError::Sv1MessageTooLong => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::UnexpectedMessage => todo!(),
        TproxyError::JobNotFound => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::InvalidMerkleRoot => {
            send_status(sender, e, error_handling::ErrorBranch::Break).await
        }
        TproxyError::Shutdown => {
            send_status(sender, e, error_handling::ErrorBranch::Continue).await
        }
        TproxyError::General(_) => {
            send_status(sender, e, error_handling::ErrorBranch::Continue).await
        }
    }
}
