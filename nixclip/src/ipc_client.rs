use std::path::{Path, PathBuf};

use gtk4::glib;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use nixclip_core::config::Config;
use nixclip_core::ipc::{self, ClientMessage, ServerMessage};
use nixclip_core::{ContentClass, EntryId, RestoreMode};

struct IpcRequest {
    message: ClientMessage,
    reply: tokio::sync::oneshot::Sender<Result<ServerMessage, String>>,
}

pub struct UiIpcClient {
    request_tx: mpsc::UnboundedSender<IpcRequest>,
}

impl UiIpcClient {
    pub fn new(socket_path: &Path) -> Self {
        let (request_tx, request_rx) = mpsc::unbounded_channel::<IpcRequest>();
        let path = socket_path.to_path_buf();

        std::thread::Builder::new()
            .name("nixclip-ipc".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime for IPC");
                rt.block_on(ipc_loop(path, request_rx));
            })
            .expect("failed to spawn IPC thread");

        Self { request_tx }
    }

    pub fn query(
        &self,
        text: Option<String>,
        class: Option<ContentClass>,
        limit: u32,
        callback: impl Fn(Result<(Vec<nixclip_core::EntrySummary>, u32), String>) + 'static,
    ) {
        let msg = ClientMessage::query(text, class.map(|c| c.as_str().to_string()), 0, limit);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::QueryResult { entries, total, .. }) => callback(Ok((entries, total))),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    pub fn restore(
        &self,
        id: EntryId,
        mode: RestoreMode,
        callback: impl Fn(Result<(), String>) + 'static,
    ) {
        let msg = ClientMessage::restore(id, mode);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::RestoreResult { success, error, .. }) => {
                if success {
                    callback(Ok(()));
                } else {
                    callback(Err(error.unwrap_or_else(|| "restore failed".into())));
                }
            }
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    pub fn delete(&self, id: EntryId, callback: impl Fn(Result<(), String>) + 'static) {
        let msg = ClientMessage::delete(vec![id]);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::Ok { .. }) => callback(Ok(())),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    pub fn pin(&self, id: EntryId, pinned: bool, callback: impl Fn(Result<(), String>) + 'static) {
        let msg = ClientMessage::pin(id, pinned);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::Ok { .. }) => callback(Ok(())),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    pub fn clear_unpinned(&self, callback: impl Fn(Result<(), String>) + 'static) {
        let msg = ClientMessage::clear_unpinned();

        self.send(msg, move |res| match res {
            Ok(ServerMessage::Ok { .. }) => callback(Ok(())),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    pub fn get_config(&self, callback: impl Fn(Result<Config, String>) + 'static) {
        let msg = ClientMessage::get_config();

        self.send(msg, move |res| match res {
            Ok(ServerMessage::ConfigValue { config, .. }) => callback(Ok(config)),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    pub fn set_config(&self, config: Config, callback: impl Fn(Result<Config, String>) + 'static) {
        let patch = match toml::Value::try_from(config) {
            Ok(value) => value,
            Err(error) => {
                callback(Err(format!("failed to serialize config: {error}")));
                return;
            }
        };
        let msg = ClientMessage::set_config(patch);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::ConfigValue { config, .. }) => callback(Ok(config)),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    fn send(
        &self,
        message: ClientMessage,
        callback: impl Fn(Result<ServerMessage, String>) + 'static,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<ServerMessage, String>>();

        glib::spawn_future_local(async move {
            if let Ok(result) = rx.await {
                callback(result);
            }
        });

        let req = IpcRequest { message, reply: tx };

        if self.request_tx.send(req).is_err() {
            tracing::error!("IPC background thread has exited; cannot send request");
        }
    }
}

async fn ipc_loop(socket_path: PathBuf, mut rx: mpsc::UnboundedReceiver<IpcRequest>) {
    let mut stream: Option<UnixStream> = None;

    while let Some(req) = rx.recv().await {
        let conn = match &mut stream {
            Some(s) => s,
            None => match UnixStream::connect(&socket_path).await {
                Ok(s) => {
                    stream = Some(s);
                    stream.as_mut().expect("stream just inserted")
                }
                Err(e) => {
                    let msg = format!("cannot connect to daemon: {e}");
                    tracing::warn!("{}", msg);
                    let _ = req.reply.send(Err(msg));
                    continue;
                }
            },
        };

        let (mut reader, mut writer) = conn.split();

        if let Err(e) = ipc::send_message(&mut writer, &req.message).await {
            let msg = format!("failed to send IPC message: {e}");
            tracing::warn!("{}", msg);
            let _ = req.reply.send(Err(msg));
            stream = None;
            continue;
        }

        match ipc::recv_message::<_, ServerMessage>(&mut reader).await {
            Ok(response) => {
                let _ = req.reply.send(Ok(response));
            }
            Err(e) => {
                let msg = format!("failed to read IPC response: {e}");
                tracing::warn!("{}", msg);
                let _ = req.reply.send(Err(msg));
                stream = None;
            }
        }
    }

    tracing::debug!("IPC background loop exiting");
}
