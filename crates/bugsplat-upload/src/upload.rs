use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bugsplat_core::{Error, Result};
use futures_util::TryStreamExt;
use futures_util::future::try_join_all;
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;

use crate::client::{Client, Failure, FileFields, Target};
use crate::compress::{self, Compressed};
use crate::plan::{Route, SymbolFile};
use crate::retry::{Backoff, RateLimitGate, Retry, RetryPolicy};
use crate::time::iso8601;

/// Receives progress as it happens. Called from the upload's own task and from request bodies.
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

/// `index` is the file's position in the list given to [`upload`].
#[derive(Debug, Clone)]
pub enum Event {
    Skipped {
        index: usize,
        name: String,
        reason: String,
    },
    /// The file is compressed and about to be sent; `bytes` is what goes over the wire.
    Started {
        index: usize,
        name: String,
        bytes: u64,
    },
    /// Bytes of the current attempt handed to the connection so far.
    Progress { index: usize, sent: u64 },
    Uploaded {
        index: usize,
        name: String,
        bytes: u64,
        elapsed: Duration,
    },
    Retrying {
        index: usize,
        name: String,
        error: String,
        delay: Duration,
    },
    /// The server answered 429 and every worker is waiting.
    RateLimited { pause: Duration },
}

#[derive(Debug, Clone)]
pub struct Options {
    pub workers: usize,
    pub retry: RetryPolicy,
    /// What each worker waits between two files, as `@bugsplat/js-api-client` does, to stay
    /// under the server's rate limit.
    pub pause_between_files: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            workers: default_workers(),
            retry: RetryPolicy::default(),
            pause_between_files: Duration::from_secs(1),
        }
    }
}

/// The number of CPU cores or 8, whichever is larger: uploads wait on the network more than
/// on the CPU.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .max(8)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileStatus {
    Uploaded,
    Skipped,
    WouldUpload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    #[serde(flatten)]
    pub file: SymbolFile,
    pub status: FileStatus,
    /// Compressed bytes sent; 0 for a skipped file.
    pub uploaded_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub uploaded: usize,
    pub skipped: usize,
    /// Compressed bytes sent.
    pub bytes: u64,
    pub seconds: f64,
    pub bytes_per_second: f64,
    pub files: Vec<FileResult>,
}

impl Summary {
    /// What [`upload`] would do with a plan, in the shape of what it did.
    pub fn dry_run(files: &[SymbolFile]) -> Self {
        let files: Vec<FileResult> = files
            .iter()
            .map(|file| FileResult {
                file: file.clone(),
                status: if file.route == Route::Skip {
                    FileStatus::Skipped
                } else {
                    FileStatus::WouldUpload
                },
                uploaded_bytes: 0,
            })
            .collect();
        Self {
            uploaded: 0,
            skipped: files
                .iter()
                .filter(|file| file.status == FileStatus::Skipped)
                .count(),
            bytes: 0,
            seconds: 0.0,
            bytes_per_second: 0.0,
            files,
        }
    }
}

/// Uploads every file of a [`plan`](crate::plan). The first file that fails for good stops
/// the run and is the error returned; files already uploaded stay uploaded.
pub async fn upload(
    client: &Client,
    target: &Target,
    files: &[SymbolFile],
    options: &Options,
    events: EventSink,
) -> Result<Summary> {
    let started = Instant::now();
    let tmp = tempfile::Builder::new()
        .prefix("bugsplat-upload-")
        .tempdir()?;
    let run = Run {
        client,
        target,
        options,
        events,
        gate: RateLimitGate::default(),
        compressors: Semaphore::new(std::thread::available_parallelism().map_or(1, usize::from)),
        tmp_dir: tmp.path(),
    };

    let mut pending = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if file.route == Route::Skip {
            (run.events)(Event::Skipped {
                index,
                name: file.name.clone(),
                reason: file
                    .skip_reason
                    .clone()
                    .unwrap_or_else(|| "it has no debug id".to_string()),
            });
        } else {
            pending.push(index);
        }
    }

    let next = AtomicUsize::new(0);
    let uploaded_bytes = Mutex::new(vec![0u64; files.len()]);
    let workers = (0..options.workers.clamp(1, pending.len().max(1))).map(|_| async {
        let mut first = true;
        while let Some(&index) = pending.get(next.fetch_add(1, Ordering::Relaxed)) {
            if !first {
                tokio::time::sleep(options.pause_between_files).await;
            }
            first = false;
            let file = &files[index];
            let bytes = run
                .upload_one(index, file)
                .await
                .map_err(|error| with_file(error, &file.path))?;
            uploaded_bytes.lock().expect("uploaded bytes lock")[index] = bytes;
        }
        Ok::<(), Error>(())
    });
    try_join_all(workers).await?;

    let uploaded_bytes = uploaded_bytes.into_inner().expect("uploaded bytes lock");
    let files: Vec<FileResult> = files
        .iter()
        .zip(uploaded_bytes)
        .map(|(file, uploaded_bytes)| FileResult {
            file: file.clone(),
            status: if file.route == Route::Skip {
                FileStatus::Skipped
            } else {
                FileStatus::Uploaded
            },
            uploaded_bytes,
        })
        .collect();
    let bytes = files.iter().map(|file| file.uploaded_bytes).sum();
    let seconds = started.elapsed().as_secs_f64();
    Ok(Summary {
        uploaded: pending.len(),
        skipped: files.len() - pending.len(),
        bytes,
        seconds,
        bytes_per_second: if seconds > 0.0 {
            bytes as f64 / seconds
        } else {
            0.0
        },
        files,
    })
}

struct Run<'a> {
    client: &'a Client,
    target: &'a Target,
    options: &'a Options,
    events: EventSink,
    gate: RateLimitGate,
    compressors: Semaphore,
    tmp_dir: &'a Path,
}

enum Payload {
    Compressed(Compressed),
    /// A `.zip` uploads as it is.
    AsIs {
        path: PathBuf,
        size: u64,
    },
}

impl Payload {
    fn path(&self) -> &Path {
        match self {
            Payload::Compressed(compressed) => &compressed.path,
            Payload::AsIs { path, .. } => path,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Payload::Compressed(compressed) => compressed.size,
            Payload::AsIs { size, .. } => *size,
        }
    }
}

impl Run<'_> {
    async fn upload_one(&self, index: usize, file: &SymbolFile) -> Result<u64> {
        let metadata = tokio::fs::metadata(&file.path).await?;
        let last_modified = iso8601(metadata.modified().unwrap_or_else(|_| SystemTime::now()));
        let payload = self.payload(file, metadata.len()).await?;
        let bytes = payload.size();
        let fields = FileFields {
            name: &file.name,
            module_name: &file.module_name,
            debug_id: &file.debug_id,
            // The symbol server records the size before compression; the versions endpoint
            // records the size of the zip it stores.
            size: match (&payload, file.route) {
                (Payload::Compressed(compressed), Route::SymbolServer) => compressed.source_size,
                _ => bytes,
            },
            last_modified: &last_modified,
        };

        (self.events)(Event::Started {
            index,
            name: file.name.clone(),
            bytes,
        });
        let started = Instant::now();
        let policy = &self.options.retry;
        let mut backoff = Backoff::default();
        let mut retries = 0;
        loop {
            let failure = match self.attempt(index, file, &fields, &payload).await {
                Ok(()) => break,
                Err(failure) => failure,
            };
            if failure.retry == Retry::No || retries >= policy.max_retries {
                return Err(failure.error);
            }
            if let Retry::RateLimited(retry_after) = failure.retry {
                let pause = policy.pause_for(retry_after);
                if self.gate.pause(pause) {
                    (self.events)(Event::RateLimited { pause });
                }
            }
            let delay = backoff.next(policy);
            retries += 1;
            (self.events)(Event::Retrying {
                index,
                name: file.name.clone(),
                error: failure.error.to_string(),
                delay,
            });
            tokio::time::sleep(delay).await;
        }
        (self.events)(Event::Uploaded {
            index,
            name: file.name.clone(),
            bytes,
            elapsed: started.elapsed(),
        });
        Ok(bytes)
    }

    async fn payload(&self, file: &SymbolFile, file_size: u64) -> Result<Payload> {
        let gzip = file.route == Route::SymbolServer;
        if !gzip && compress::is_zip(&file.path) {
            return Ok(Payload::AsIs {
                path: file.path.clone(),
                size: file_size,
            });
        }
        let _permit = self
            .compressors
            .acquire()
            .await
            .map_err(|error| Error::Other(error.to_string()))?;
        let (path, slice, tmp_dir) = (file.path.clone(), file.slice, self.tmp_dir.to_path_buf());
        let entry_name = file_name(&file.path);
        let compressed = tokio::task::spawn_blocking(move || {
            if gzip {
                compress::gzip(&path, slice, &tmp_dir)
            } else {
                compress::zip(&path, &entry_name, &tmp_dir)
            }
        })
        .await
        .map_err(|error| Error::Other(error.to_string()))??;
        Ok(Payload::Compressed(compressed))
    }

    /// One try at the whole exchange for a file. A retry starts over from the upload URL,
    /// because a presigned URL can expire while the file waits.
    async fn attempt(
        &self,
        index: usize,
        file: &SymbolFile,
        fields: &FileFields<'_>,
        payload: &Payload,
    ) -> std::result::Result<(), Failure> {
        let symbol_server = file.route == Route::SymbolServer;

        self.gate.wait().await;
        let url = if symbol_server {
            self.client
                .symbol_server_upload_url(self.target, fields)
                .await?
        } else {
            self.client.versions_upload_url(self.target, fields).await?
        };

        self.gate.wait().await;
        let body = self.body(index, payload.path()).await?;
        self.client
            .put(&url, &file.name, body, payload.size(), symbol_server)
            .await?;

        if symbol_server {
            self.gate.wait().await;
            self.client
                .symbol_server_upload_complete(self.target, fields)
                .await?;
        }
        Ok(())
    }

    async fn body(&self, index: usize, path: &Path) -> std::result::Result<reqwest::Body, Failure> {
        let file = tokio::fs::File::open(path).await.map_err(|error| Failure {
            error: Error::Io(error),
            retry: Retry::No,
        })?;
        let events = Arc::clone(&self.events);
        let mut sent = 0u64;
        let stream = ReaderStream::with_capacity(file, 64 * 1024).inspect_ok(move |chunk| {
            sent += chunk.len() as u64;
            events(Event::Progress { index, sent });
        });
        Ok(reqwest::Body::wrap_stream(stream))
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn with_file(error: Error, path: &Path) -> Error {
    let message = format!("Failed to upload {}: {error}", path.display());
    match error {
        Error::Api { status, .. } => Error::Api { status, message },
        _ => Error::Other(message),
    }
}
