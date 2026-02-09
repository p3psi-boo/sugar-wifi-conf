use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{self, MissedTickBehavior};

use crate::config::CustomInfoItem;
use crate::exec::{self, ExecOptions};
use crate::uuid;

pub const DEFAULT_CUSTOM_INFO_INTERVAL: Duration = Duration::from_secs(10);
pub const DEFAULT_CUSTOM_INFO_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_CUSTOM_INFO_MAX_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomInfoSpec {
    /// 1-based index (0001 == first info item).
    pub index: u16,
    /// 4 hex digits, uppercase.
    pub suffix: Arc<str>,
    pub label: Arc<str>,
    pub command: Arc<str>,
    pub interval: Duration,
}

#[derive(Debug, Clone)]
pub struct CustomInfoRegistry {
    items: Vec<CustomInfoSpec>,
    by_suffix: HashMap<u16, usize>,
}

impl CustomInfoRegistry {
    pub fn from_config(items: &[CustomInfoItem]) -> anyhow::Result<Self> {
        let mut out: Vec<CustomInfoSpec> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let index = u16::try_from(i + 1)
                .map_err(|_| anyhow::anyhow!("custom info count exceeds u16::MAX"))?;
            let interval = item
                .interval
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_CUSTOM_INFO_INTERVAL);
            out.push(CustomInfoSpec {
                index,
                suffix: uuid::suffix_for_index0(i).into(),
                label: item.label.clone().into(),
                command: item.command.clone().into(),
                interval,
            });
        }

        let mut by_suffix = HashMap::new();
        for (i, spec) in out.iter().enumerate() {
            by_suffix.insert(spec.index, i);
        }
        Ok(Self { items: out, by_suffix })
    }

    pub fn items(&self) -> &[CustomInfoSpec] {
        &self.items
    }

    pub fn get_by_suffix(&self, suffix: &str) -> Option<&CustomInfoSpec> {
        let idx = suffix_to_u16(suffix)?;
        self.by_suffix.get(&idx).and_then(|i| self.items.get(*i))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomInfoUpdate {
    pub index: u16,
    pub suffix: Arc<str>,
    pub output: Arc<[u8]>,
}

/// Spawns one polling task per custom-info item.
///
/// Each task executes its command every `interval` and sends an update whenever
/// output bytes change (including an initial update).
pub struct CustomInfoRunner {
    rx: mpsc::Receiver<CustomInfoUpdate>,
    handles: Vec<JoinHandle<()>>,
    shutdown: watch::Sender<bool>,
}

impl CustomInfoRunner {
    pub fn spawn(registry: CustomInfoRegistry, opts: Option<ExecOptions>) -> Self {
        let opts = opts.unwrap_or_else(|| ExecOptions {
            timeout: DEFAULT_CUSTOM_INFO_TIMEOUT,
            max_output_bytes: DEFAULT_CUSTOM_INFO_MAX_BYTES,
        });

        // Buffer a handful of updates; BLE side may be slow.
        let (tx, rx) = mpsc::channel::<CustomInfoUpdate>(32);
        let mut handles = Vec::new();
        let (shutdown, shutdown_rx) = watch::channel(false);

        for spec in registry.items {
            let tx = tx.clone();
            let opts = opts.clone();
            let mut shutdown_rx = shutdown_rx.clone();

            handles.push(tokio::spawn(async move {
                let mut ticker = time::interval(spec.interval);
                ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

                let mut last: Option<Arc<[u8]>> = None;

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {},
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                    }

                    let mut out = match exec::run("sh", ["-c", spec.command.as_ref()], opts.clone()).await {
                        Ok(v) => {
                            let mut bytes = v.stdout;
                            bytes.extend_from_slice(&v.stderr);
                            bytes
                        }
                        Err(e) => format!("cmd error: {e}").into_bytes(),
                    };
                    if out.len() > opts.max_output_bytes {
                        out.truncate(opts.max_output_bytes);
                    }

                    // Node sends raw Buffer. Keep bytes as-is, but trim trailing NULs.
                    while out.last() == Some(&0) {
                        out.pop();
                    }

                    let out: Arc<[u8]> = out.into();
                    if last.as_ref().map_or(true, |prev| prev.as_ref() != out.as_ref()) {
                        last = Some(out.clone());
                        if tx
                            .send(CustomInfoUpdate {
                                index: spec.index,
                                suffix: spec.suffix.clone(),
                                output: out,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }));
        }

        Self {
            rx,
            handles,
            shutdown,
        }
    }

    pub async fn recv(&mut self) -> Option<CustomInfoUpdate> {
        self.rx.recv().await
    }
}

impl Drop for CustomInfoRunner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for h in &self.handles {
            h.abort();
        }
    }
}

fn suffix_to_u16(suffix: &str) -> Option<u16> {
    if suffix.len() != 4 {
        return None;
    }
    u16::from_str_radix(suffix, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid;

    fn info(label: &str, command: &str, interval: Option<u64>) -> CustomInfoItem {
        CustomInfoItem {
            label: label.to_string(),
            command: command.to_string(),
            interval,
        }
    }

    #[test]
    fn suffix_is_1_based_and_uppercase() {
        assert_eq!(uuid::suffix_for_index0(0), "0001");
        assert_eq!(uuid::suffix_for_index0(15), "0010");
    }

    #[test]
    fn registry_assigns_suffix_and_default_interval() {
        let reg = CustomInfoRegistry::from_config(&[
            info("a", "echo a", None),
            info("b", "echo b", Some(1)),
        ])
        .unwrap();

        assert_eq!(reg.items()[0].index, 1);
        assert_eq!(reg.items()[0].suffix.as_ref(), "0001");
        assert_eq!(reg.items()[0].interval, DEFAULT_CUSTOM_INFO_INTERVAL);

        assert_eq!(reg.items()[1].index, 2);
        assert_eq!(reg.items()[1].suffix.as_ref(), "0002");
        assert_eq!(reg.items()[1].interval, Duration::from_secs(1));

        assert_eq!(reg.get_by_suffix("0001").unwrap().label.as_ref(), "a");
        assert_eq!(reg.get_by_suffix("0002").unwrap().label.as_ref(), "b");
        assert!(reg.get_by_suffix("0003").is_none());
    }
}
