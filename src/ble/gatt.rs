use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Context;
use tracing::info;
use zbus::zvariant::{Array, ObjectPath, OwnedObjectPath, OwnedValue, Str, Type, Value};
use zbus::Connection;

use std::sync::Mutex as StdMutex;

use crate::config::{AppConfig, CustomConfigFile};
use crate::proto;
use crate::uuid;

/// `GetManagedObjects` return type: a{oa{sa{sv}}}
pub type ManagedObjects =
    HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// Root object for the GATT application.
/// BlueZ requires `org.freedesktop.DBus.ObjectManager` at the application root.
#[derive(Clone)]
pub struct ObjectManagerRoot {
    service_path: OwnedObjectPath,
    service_uuid: String,
    service_primary: bool,
    characteristics: Vec<CharacteristicMeta>,
}

impl ObjectManagerRoot {
    fn new(
        service_path: OwnedObjectPath,
        service_uuid: String,
        service_primary: bool,
        characteristics: Vec<CharacteristicMeta>,
    ) -> Self {
        Self {
            service_path,
            service_uuid,
            service_primary,
            characteristics,
        }
    }
}

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl ObjectManagerRoot {
    fn get_managed_objects(&self) -> ManagedObjects {
        build_managed_objects(
            &self.service_path,
            &self.service_uuid,
            self.service_primary,
            &self.characteristics,
        )
    }
}

#[derive(Clone)]
struct CharacteristicMeta {
    path: OwnedObjectPath,
    uuid: String,
    flags: Vec<String>,
    notifying: bool,
    value: Vec<u8>,
}

fn build_managed_objects(
    service_path: &OwnedObjectPath,
    service_uuid: &str,
    service_primary: bool,
    characteristics: &[CharacteristicMeta],
) -> ManagedObjects {
    let mut mo: ManagedObjects = HashMap::new();

    // org.bluez.GattService1 properties
    let mut svc_ifaces: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
    let mut svc_props: HashMap<String, OwnedValue> = HashMap::new();
    svc_props.insert("UUID".to_string(), ov(Value::from(service_uuid.to_string())));
    svc_props.insert("Primary".to_string(), OwnedValue::from(service_primary));
    svc_props.insert(
        "Includes".to_string(),
        // We don't include any nested services in this minimal skeleton.
        empty_object_path_array(),
    );
    svc_ifaces.insert("org.bluez.GattService1".to_string(), svc_props);
    mo.insert(service_path.clone(), svc_ifaces);

    // org.bluez.GattCharacteristic1 properties
    let svc_obj_path = ObjectPath::try_from(service_path.as_str())
        .expect("service path must be a valid ObjectPath");

    for chr in characteristics {
        let mut ch_ifaces: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
        let mut ch_props: HashMap<String, OwnedValue> = HashMap::new();
        ch_props.insert("UUID".to_string(), ov(Value::from(chr.uuid.clone())));
        ch_props.insert("Service".to_string(), OwnedValue::from(svc_obj_path.clone()));
        ch_props.insert("Flags".to_string(), string_array(&chr.flags));
        // BlueZ expects these properties for notify-capable characteristics.
        // Keep them present even for read-only characteristics.
        ch_props.insert("Notifying".to_string(), OwnedValue::from(chr.notifying));
        // `Value` is an array of bytes (`ay`).
        ch_props.insert("Value".to_string(), ov(Value::from(chr.value.clone())));
        ch_props.insert(
            "Descriptors".to_string(),
            // We don't use descriptors in this minimal skeleton.
            empty_object_path_array(),
        );
        ch_ifaces.insert("org.bluez.GattCharacteristic1".to_string(), ch_props);
        mo.insert(chr.path.clone(), ch_ifaces);
    }

    mo
}

fn ov(v: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(v).expect("zvariant OwnedValue conversion")
}

fn empty_object_path_array() -> OwnedValue {
    let arr = Array::new(ObjectPath::signature());
    OwnedValue::try_from(arr).expect("empty object-path array conversion")
}

fn string_array(strings: &[String]) -> OwnedValue {
    // This creates a D-Bus array of strings (`as`).
    let mut arr = Array::new(Str::signature());
    for s in strings {
        // Str is a signature-aware wrapper for &str.
        let st = Str::from(s.as_str());
        arr.append(Value::from(st))
            .expect("array element signature mismatch");
    }
    OwnedValue::try_from(arr).expect("string array conversion")
}

#[derive(Clone)]
pub struct GattService {
    uuid: String,
    primary: bool,
    includes: Vec<OwnedObjectPath>,
}

#[zbus::interface(name = "org.bluez.GattService1")]
impl GattService {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> &str {
        &self.uuid
    }

    #[zbus(property, name = "Primary")]
    fn primary(&self) -> bool {
        self.primary
    }

    #[zbus(property, name = "Includes")]
    fn includes(&self) -> Vec<OwnedObjectPath> {
        self.includes.clone()
    }
}

pub trait ReadableCharacteristic: Send + Sync {
    fn read(&self) -> Vec<u8>;
}

impl<F> ReadableCharacteristic for F
where
    F: Fn() -> Vec<u8> + Send + Sync,
{
    fn read(&self) -> Vec<u8> {
        (self)()
    }
}

pub type BoxWriteFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>;

pub trait WritableCharacteristic: Send + Sync {
    fn write(&self, value: Vec<u8>) -> BoxWriteFuture;
}

impl<F, Fut> WritableCharacteristic for F
where
    F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    fn write(&self, value: Vec<u8>) -> BoxWriteFuture {
        Box::pin((self)(value))
    }
}

#[derive(Debug, Clone)]
struct CharacteristicState {
    value: Arc<[u8]>,
    notifying: bool,
}

struct CharacteristicInner {
    path: OwnedObjectPath,
    conn: OnceLock<Connection>,
    state: StdMutex<CharacteristicState>,
    reader: StdMutex<Option<Arc<dyn ReadableCharacteristic>>>,
    writer: StdMutex<Option<Arc<dyn WritableCharacteristic>>>,

    // Node parity needs "start work on subscribe, stop on unsubscribe".
    notify_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    on_start_notify: StdMutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    on_stop_notify: StdMutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl CharacteristicInner {
    fn bind_connection(&self, conn: &Connection) {
        // Export happens once; this allows cloned characteristic instances to share the same
        // connection for emitting PropertiesChanged.
        let _ = self.conn.get_or_init(|| conn.clone());
    }

    fn set_reader(&self, reader: Option<Arc<dyn ReadableCharacteristic>>) {
        *self.reader.lock().unwrap() = reader;
    }

    fn set_writer(&self, writer: Option<Arc<dyn WritableCharacteristic>>) {
        *self.writer.lock().unwrap() = writer;
    }

    fn set_on_start_notify(&self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.on_start_notify.lock().unwrap() = cb;
    }

    fn set_on_stop_notify(&self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.on_stop_notify.lock().unwrap() = cb;
    }

    fn call_on_start_notify(&self) {
        if let Some(cb) = self.on_start_notify.lock().unwrap().clone() {
            cb();
        }
    }

    fn call_on_stop_notify(&self) {
        if let Some(cb) = self.on_stop_notify.lock().unwrap().clone() {
            cb();
        }
    }

    fn spawn_notify_task(&self, handle: tokio::task::JoinHandle<()>) {
        let mut guard = self.notify_task.lock().unwrap();
        if let Some(old) = guard.take() {
            old.abort();
        }
        *guard = Some(handle);
    }

    fn stop_notify_task(&self) {
        if let Some(old) = self.notify_task.lock().unwrap().take() {
            old.abort();
        }
    }

    fn notifying(&self) -> bool {
        self.state.lock().unwrap().notifying
    }

    fn value(&self) -> Vec<u8> {
        self.state.lock().unwrap().value.as_ref().to_vec()
    }

    async fn set_notifying(&self, notifying: bool) -> anyhow::Result<()> {
        {
            let mut st = self.state.lock().unwrap();
            st.notifying = notifying;
        }
        self.emit_properties_changed(vec![("Notifying", OwnedValue::from(notifying))])
            .await
    }

    async fn set_value(&self, value: Vec<u8>) -> anyhow::Result<()> {
        let value: Arc<[u8]> = value.into();
        {
            let mut st = self.state.lock().unwrap();
            st.value = value.clone();
        }
        // `Value` is an array of bytes (`ay`).
        self.emit_properties_changed(vec![("Value", ov(Value::from(value.as_ref().to_vec())))])
            .await
    }

    async fn emit_properties_changed(
        &self,
        changed: Vec<(&'static str, OwnedValue)>,
    ) -> anyhow::Result<()> {
        let Some(conn) = self.conn.get() else {
            // Emitting before export is a logic error in the caller.
            anyhow::bail!("characteristic not exported yet (no D-Bus connection bound)");
        };

        let obj_path = ObjectPath::try_from(self.path.as_str())
            .context("invalid characteristic object path")?;

        let mut changed_props: HashMap<String, OwnedValue> = HashMap::new();
        for (k, v) in changed {
            changed_props.insert(k.to_string(), v);
        }

        // Signal signature: (sa{sv}as)
        // - interface name whose properties changed
        // - dict of changed properties
        // - list of invalidated property names (unused here)
        conn.emit_signal(
            None::<&str>,
            obj_path,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.bluez.GattCharacteristic1".to_string(),
                changed_props,
                Vec::<String>::new(),
            ),
        )
        .await
        .context("emit PropertiesChanged")?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct CharacteristicHandle {
    uuid: String,
    inner: Arc<CharacteristicInner>,
}

impl CharacteristicHandle {
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn path(&self) -> &OwnedObjectPath {
        &self.inner.path
    }

    pub fn notifying(&self) -> bool {
        self.inner.notifying()
    }

    pub fn set_read_handler(&self, reader: Option<Arc<dyn ReadableCharacteristic>>) {
        self.inner.set_reader(reader);
    }

    pub fn set_write_handler(&self, writer: Option<Arc<dyn WritableCharacteristic>>) {
        self.inner.set_writer(writer);
    }

    pub fn on_start_notify<F>(&self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.set_on_start_notify(Some(Arc::new(f)));
    }

    pub fn on_stop_notify<F>(&self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.set_on_stop_notify(Some(Arc::new(f)));
    }

    pub fn spawn_notify_task<Fut>(&self, fut: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let h = tokio::spawn(fut);
        self.inner.spawn_notify_task(h);
    }

    pub fn stop_notify_task(&self) {
        self.inner.stop_notify_task();
    }

    /// Update the characteristic Value property (and emit PropertiesChanged).
    pub async fn set_value(&self, value: Vec<u8>) -> anyhow::Result<()> {
        self.inner.set_value(value).await
    }

    /// Notify a framed message by chunking to 20 bytes and sleeping between chunks.
    ///
    /// Compatibility rule: do not rely on negotiated MTU.
    pub async fn notify_message(&self, message: &[u8]) -> anyhow::Result<()> {
        use std::time::Duration;

        let chunks = proto::frame_notify_message(message);
        let total = chunks.len();
        for (idx, chunk) in chunks.into_iter().enumerate() {
            self.inner.set_value(chunk).await?;
            if idx + 1 < total {
                tokio::time::sleep(Duration::from_millis(proto::NOTIFY_CHUNK_DELAY_MS)).await;
            }
        }
        Ok(())
    }

    pub fn on_read<F>(&self, f: F)
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
    {
        self.set_read_handler(Some(Arc::new(f)));
    }

    pub fn on_write<F, Fut>(&self, f: F)
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.set_write_handler(Some(Arc::new(f)));
    }
}

#[derive(Clone)]
pub struct GattCharacteristic {
    uuid: String,
    service: OwnedObjectPath,
    flags: Vec<String>,
    descriptors: Vec<OwnedObjectPath>,
    inner: Arc<CharacteristicInner>,
}

#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl GattCharacteristic {
    async fn read_value(&self, _options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<Vec<u8>> {
        // Prefer the registered read handler; otherwise fall back to cached Value.
        let reader = self.inner.reader.lock().unwrap().clone();
        match reader {
            Some(r) => Ok(r.read()),
            None => Ok(self.inner.value()),
        }
    }

    async fn write_value(
        &self,
        value: Vec<u8>,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let writer = self.inner.writer.lock().unwrap().clone();
        let Some(w) = writer else {
            return Err(zbus::fdo::Error::Failed("write not supported".into()));
        };

        w.write(value.clone())
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        self.inner
            .set_value(value)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        Ok(())
    }

    async fn start_notify(&self) -> zbus::fdo::Result<()> {
        self.inner
            .set_notifying(true)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

        self.inner.call_on_start_notify();
        Ok(())
    }

    async fn stop_notify(&self) -> zbus::fdo::Result<()> {
        // Stop any subscription-driven work first (Node clears interval immediately).
        self.inner.stop_notify_task();
        self.inner.call_on_stop_notify();

        self.inner
            .set_notifying(false)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> &str {
        &self.uuid
    }

    #[zbus(property, name = "Service")]
    fn service(&self) -> OwnedObjectPath {
        self.service.clone()
    }

    #[zbus(property, name = "Flags")]
    fn flags(&self) -> Vec<String> {
        self.flags.clone()
    }

    #[zbus(property, name = "Descriptors")]
    fn descriptors(&self) -> Vec<OwnedObjectPath> {
        self.descriptors.clone()
    }

    #[zbus(property, name = "Notifying")]
    fn notifying(&self) -> bool {
        self.inner.notifying()
    }

    #[zbus(property, name = "Value")]
    fn value(&self) -> Vec<u8> {
        self.inner.value()
    }
}

#[derive(Clone)]
pub struct GattHandle {
    by_uuid: Arc<HashMap<String, CharacteristicHandle>>,
    by_path: Arc<HashMap<String, CharacteristicHandle>>,
}

impl GattHandle {
    pub fn characteristic_by_uuid(&self, uuid: &str) -> Option<CharacteristicHandle> {
        let key = uuid::bluez_uuid(uuid).ok()?;
        self.by_uuid.get(&key).cloned()
    }

    pub fn characteristic_by_path(&self, path: &str) -> Option<CharacteristicHandle> {
        self.by_path.get(path).cloned()
    }

    pub fn on_read_uuid<F>(&self, uuid: &str, f: F) -> anyhow::Result<()>
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
    {
        let Some(ch) = self.characteristic_by_uuid(uuid) else {
            anyhow::bail!("unknown characteristic uuid: {uuid}");
        };
        ch.on_read(f);
        Ok(())
    }

    pub fn on_write_uuid<F, Fut>(&self, uuid: &str, f: F) -> anyhow::Result<()>
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let Some(ch) = self.characteristic_by_uuid(uuid) else {
            anyhow::bail!("unknown characteristic uuid: {uuid}");
        };
        ch.on_write(f);
        Ok(())
    }

    pub async fn notify_uuid(&self, uuid: &str, message: &[u8]) -> anyhow::Result<()> {
        let Some(ch) = self.characteristic_by_uuid(uuid) else {
            anyhow::bail!("unknown characteristic uuid: {uuid}");
        };
        ch.notify_message(message).await
    }

    /// Send a single notification update without adding END_TAG or chunking.
    ///
    /// Node parity: some characteristics (e.g. NOTIFY_MESSAGE, WIFI_NAME, IP_ADDRESS)
    /// notify raw strings directly.
    pub async fn notify_raw_uuid(&self, uuid: &str, value: &[u8]) -> anyhow::Result<()> {
        let Some(ch) = self.characteristic_by_uuid(uuid) else {
            anyhow::bail!("unknown characteristic uuid: {uuid}");
        };
        ch.set_value(value.to_vec()).await
    }

    pub async fn notify_path(&self, path: &str, message: &[u8]) -> anyhow::Result<()> {
        let Some(ch) = self.characteristic_by_path(path) else {
            anyhow::bail!("unknown characteristic path: {path}");
        };
        ch.notify_message(message).await
    }
}

#[derive(Clone)]
pub struct Advertisement {
    local_name: String,
    service_uuids: Vec<String>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    fn release(&self) {
        info!("bluez released advertisement");
    }

    #[zbus(property, name = "Type")]
    fn type_(&self) -> &str {
        "peripheral"
    }

    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<String> {
        self.service_uuids.clone()
    }

    #[zbus(property, name = "LocalName")]
    fn local_name(&self) -> &str {
        &self.local_name
    }
}

pub struct GattApplication {
    root_path: OwnedObjectPath,
    advertisement_path: OwnedObjectPath,

    om_root: ObjectManagerRoot,
    service_path: OwnedObjectPath,
    service: GattService,
    chars: Vec<(OwnedObjectPath, GattCharacteristic)>,
    advertisement: Advertisement,

    handle: GattHandle,
}

impl GattApplication {
    pub fn new(
        cfg: &AppConfig,
        service_uuid: String,
        custom: Option<CustomConfigFile>,
    ) -> anyhow::Result<Self> {
        let root_path = OwnedObjectPath::try_from("/com/sugar/wifi_conf")
            .context("invalid root object path")?;
        let advertisement_path = OwnedObjectPath::try_from("/com/sugar/wifi_conf/advertisement0")
            .context("invalid advertisement object path")?;

        let service_path = OwnedObjectPath::try_from("/com/sugar/wifi_conf/service0")
            .context("invalid service object path")?;

        let service = GattService {
            uuid: service_uuid.clone(),
            primary: true,
            includes: Vec::new(),
        };

        // Node parity: export characteristics based on the optional custom config.
        // Custom info/command characteristics only exist if the JSON loads.

        let device_model = read_device_model();
        let service_name_val = cfg.name.clone();

        let service_name_reader: Arc<dyn ReadableCharacteristic> =
            Arc::new(move || service_name_val.as_bytes().to_vec());
        let device_model_reader: Arc<dyn ReadableCharacteristic> =
            Arc::new(move || device_model.as_bytes().to_vec());

        let noop_writer: Arc<dyn WritableCharacteristic> = Arc::new(|_value: Vec<u8>| async move {
            // Real behavior is attached by the caller via `GattHandle::on_write_uuid()`.
            Ok(())
        });

        let mut chars: Vec<(OwnedObjectPath, GattCharacteristic)> = Vec::new();
        let mut char_index: usize = 0;

        let mut add_char = |uuid_raw: String,
                            flags: Vec<&str>,
                            reader: Option<Arc<dyn ReadableCharacteristic>>,
                            writer: Option<Arc<dyn WritableCharacteristic>>|
         -> anyhow::Result<()> {
            let path_s = format!("/com/sugar/wifi_conf/service0/char{char_index}");
            char_index += 1;
            let path = OwnedObjectPath::try_from(path_s.as_str()).context("invalid characteristic object path")?;
            let c = new_characteristic(
                path.clone(),
                uuid::bluez_uuid(&uuid_raw)?,
                service_path.clone(),
                flags.into_iter().map(|s| s.to_string()).collect(),
                reader,
                writer,
            );
            chars.push((path, c));
            Ok(())
        };

        // Core characteristics (match Node index.js order/semantics).
        let core_chars = [
            (
                uuid::SERVICE_NAME.to_string(),
                vec!["read"],
                Some(service_name_reader.clone()),
                None,
            ),
            (
                uuid::DEVICE_MODEL.to_string(),
                vec!["read"],
                Some(device_model_reader.clone()),
                None,
            ),
            (uuid::WIFI_NAME.to_string(), vec!["notify"], None, None),
            (uuid::IP_ADDRESS.to_string(), vec!["notify"], None, None),
            (
                uuid::INPUT.to_string(),
                vec!["write", "write-without-response"],
                None,
                Some(noop_writer.clone()),
            ),
            (
                uuid::INPUT_SEP.to_string(),
                vec!["write", "write-without-response"],
                None,
                Some(noop_writer.clone()),
            ),
            (uuid::NOTIFY_MESSAGE.to_string(), vec!["notify"], None, None),
        ];

        for (uuid_raw, flags, reader, writer) in core_chars {
            add_char(uuid_raw, flags, reader, writer)?;
        }

        // Custom info characteristics only exist if JSON loaded.
        if let Some(custom_cfg) = custom.as_ref() {
            let info_len = custom_cfg.info.len();
            let info_count_reader: Arc<dyn ReadableCharacteristic> =
                Arc::new(move || format!("{info_len}").as_bytes().to_vec());
            add_char(uuid::CUSTOM_INFO_COUNT.to_string(), vec!["read"], Some(info_count_reader), None)?;

            for (i, item) in custom_cfg.info.iter().enumerate() {
                let uuid_end = uuid::suffix_for_index0(i).to_ascii_lowercase();
                let label_uuid = format!("{}{}", uuid::CUSTOM_INFO_LABEL_PREFIX, uuid_end);
                let value_uuid = format!("{}{}", uuid::CUSTOM_INFO_PREFIX, uuid_end);

                let label = item.label.clone();
                let label_reader: Arc<dyn ReadableCharacteristic> =
                    Arc::new(move || label.as_bytes().to_vec());
                add_char(label_uuid, vec!["read"], Some(label_reader), None)?;

                // Value is notify-only. Behavior is attached on subscribe.
                add_char(value_uuid, vec!["notify"], None, None)?;
            }
        }

        // Custom command characteristics exist only when `custom_config.json` exists (Node returns [] otherwise).
        if let Some(custom_cfg) = custom.as_ref() {
            let commands_len = custom_cfg.commands.len();
            let cmd_count_reader: Arc<dyn ReadableCharacteristic> =
                Arc::new(move || format!("{commands_len}").as_bytes().to_vec());
            add_char(
                uuid::CUSTOM_COMMAND_COUNT.to_string(),
                vec!["read"],
                Some(cmd_count_reader),
                None,
            )?;

            for (i, cmd) in custom_cfg.commands.iter().enumerate() {
                let uuid12 = format!(
                    "{}{}",
                    uuid::CUSTOM_COMMAND_LABEL_PREFIX,
                    uuid::suffix_for_index0(i).to_ascii_lowercase()
                );
                let label = cmd.label.clone();
                let reader: Arc<dyn ReadableCharacteristic> = Arc::new(move || label.as_bytes().to_vec());
                add_char(uuid12, vec!["read"], Some(reader), None)?;
            }

            add_char(
                uuid::CUSTOM_COMMAND_INPUT.to_string(),
                vec!["write", "write-without-response"],
                None,
                Some(noop_writer.clone()),
            )?;
            add_char(uuid::CUSTOM_COMMAND_NOTIFY.to_string(), vec!["notify"], None, None)?;
        }

        let advertisement = Advertisement {
            local_name: cfg.name.clone(),
            service_uuids: vec![service_uuid.clone()],
        };

        let characteristics = chars
            .iter()
            .map(|(path, chr)| CharacteristicMeta {
                path: path.clone(),
                uuid: chr.uuid.clone(),
                flags: chr.flags.clone(),
                notifying: chr.inner.notifying(),
                value: chr.inner.value(),
            })
            .collect();
        let om_root = ObjectManagerRoot::new(
            service_path.clone(),
            service_uuid.clone(),
            true,
            characteristics,
        );

        let mut by_uuid: HashMap<String, CharacteristicHandle> = HashMap::new();
        let mut by_path: HashMap<String, CharacteristicHandle> = HashMap::new();
        for (path, chr) in &chars {
            let h = CharacteristicHandle {
                uuid: chr.uuid.clone(),
                inner: chr.inner.clone(),
            };
            by_uuid.insert(chr.uuid.clone(), h.clone());
            by_path.insert(path.as_str().to_string(), h);
        }
        let handle = GattHandle {
            by_uuid: Arc::new(by_uuid),
            by_path: Arc::new(by_path),
        };

        Ok(Self {
            root_path,
            advertisement_path,
            om_root,
            service_path,
            service,
            chars,
            advertisement,
            handle,
        })
    }

    pub fn handle(&self) -> GattHandle {
        self.handle.clone()
    }

    pub fn root_path(&self) -> &OwnedObjectPath {
        &self.root_path
    }

    pub fn advertisement_path(&self) -> &OwnedObjectPath {
        &self.advertisement_path
    }

    pub async fn export(&self, conn: &Connection) -> anyhow::Result<()> {
        let server = conn.object_server();

        server
            .at(self.root_path.as_str(), self.om_root.clone())
            .await
            .context("export ObjectManager root")?;

        server
            .at(self.service_path.as_str(), self.service.clone())
            .await
            .context("export GATT service")?;

        for (path, chr) in &self.chars {
            chr.inner.bind_connection(conn);
            server
                .at(path.as_str(), (*chr).clone())
                .await
                .context("export GATT characteristic")?;
        }

        server
            .at(self.advertisement_path.as_str(), self.advertisement.clone())
            .await
            .context("export advertisement")?;

        Ok(())
    }
}

fn new_characteristic(
    path: OwnedObjectPath,
    uuid: String,
    service: OwnedObjectPath,
    flags: Vec<String>,
    reader: Option<Arc<dyn ReadableCharacteristic>>,
    writer: Option<Arc<dyn WritableCharacteristic>>,
) -> GattCharacteristic {
    let initial_value = reader
        .as_ref()
        .map(|r| r.read())
        .unwrap_or_else(Vec::new);

    GattCharacteristic {
        uuid,
        service,
        flags,
        descriptors: Vec::new(),
        inner: Arc::new(CharacteristicInner {
            path,
            conn: OnceLock::new(),
            state: StdMutex::new(CharacteristicState {
                value: initial_value.into(),
                notifying: false,
            }),
            reader: StdMutex::new(reader),
            writer: StdMutex::new(writer),
            notify_task: StdMutex::new(None),
            on_start_notify: StdMutex::new(None),
            on_stop_notify: StdMutex::new(None),
        }),
    }
}

fn read_device_model() -> String {
    // Works on Raspberry Pi and many ARM SBCs.
    match std::fs::read_to_string("/proc/device-tree/model") {
        Ok(s) => s.trim_matches(|c: char| c == '\0' || c == '\n' || c == '\r').to_string(),
        Err(_) => "unknown".to_string(),
    }
}

// bluez_uuid and suffix formatting live in crate::uuid.
