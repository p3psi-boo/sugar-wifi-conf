use std::collections::HashMap;

use anyhow::Context;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
use zbus::Connection;

#[zbus::proxy(interface = "org.bluez.GattManager1", default_service = "org.bluez")]
trait GattManager1 {
    fn register_application(
        &self,
        application: ObjectPath<'_>,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;

    fn unregister_application(&self, application: ObjectPath<'_>) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.LEAdvertisingManager1", default_service = "org.bluez")]
trait LeAdvertisingManager1 {
    fn register_advertisement(
        &self,
        advertisement: ObjectPath<'_>,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;

    fn unregister_advertisement(&self, advertisement: ObjectPath<'_>) -> zbus::Result<()>;
}

fn object_path<'a>(path: &'a str, ctx: &str) -> anyhow::Result<ObjectPath<'a>> {
    ObjectPath::try_from(path).with_context(|| format!("invalid {ctx}"))
}

pub struct BluezClient<'a> {
    gatt: GattManager1Proxy<'a>,
    adv: LeAdvertisingManager1Proxy<'a>,
}

impl<'a> BluezClient<'a> {
    pub async fn new(
        conn: &'a Connection,
        adapter_path: &'a OwnedObjectPath,
    ) -> anyhow::Result<Self> {
        let gatt = GattManager1Proxy::builder(conn)
            .path(adapter_path.as_str())
            .context("set adapter path")?
            .build()
            .await
            .context("build org.bluez.GattManager1 proxy")?;

        let adv = LeAdvertisingManager1Proxy::builder(conn)
            .path(adapter_path.as_str())
            .context("set adapter path")?
            .build()
            .await
            .context("build org.bluez.LEAdvertisingManager1 proxy")?;

        Ok(Self { gatt, adv })
    }

    pub async fn register_gatt_application(
        &self,
        application_path: &OwnedObjectPath,
    ) -> anyhow::Result<()> {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let app_path = object_path(application_path.as_str(), "application object path")?;
        self.gatt
            .register_application(app_path, options)
            .await
            .context("GattManager1.RegisterApplication")?;
        Ok(())
    }

    pub async fn unregister_gatt_application(
        &self,
        application_path: &OwnedObjectPath,
    ) -> anyhow::Result<()> {
        let app_path = object_path(application_path.as_str(), "application object path")?;
        self.gatt
            .unregister_application(app_path)
            .await
            .context("GattManager1.UnregisterApplication")?;
        Ok(())
    }

    pub async fn register_advertisement(&self, advertisement_path: &OwnedObjectPath) -> anyhow::Result<()> {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let adv_path = object_path(advertisement_path.as_str(), "advertisement object path")?;
        self.adv
            .register_advertisement(adv_path, options)
            .await
            .context("LEAdvertisingManager1.RegisterAdvertisement")?;
        Ok(())
    }

    pub async fn unregister_advertisement(
        &self,
        advertisement_path: &OwnedObjectPath,
    ) -> anyhow::Result<()> {
        let adv_path = object_path(advertisement_path.as_str(), "advertisement object path")?;
        self.adv
            .unregister_advertisement(adv_path)
            .await
            .context("LEAdvertisingManager1.UnregisterAdvertisement")?;
        Ok(())
    }
}

pub async fn find_first_adapter(conn: &Connection) -> anyhow::Result<OwnedObjectPath> {
    let om = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")
        .context("set org.bluez destination")?
        .path("/")
        .context("set / path")?
        .build()
        .await
        .context("build org.bluez ObjectManager proxy")?;

    let objects = om
        .get_managed_objects()
        .await
        .context("org.bluez.GetManagedObjects")?;

    let mut fallback: Option<OwnedObjectPath> = None;
    for (path, ifaces) in objects {
        if ifaces.contains_key("org.bluez.Adapter1") {
            if path.as_str().ends_with("/hci0") {
                return Ok(path);
            }
            if fallback.is_none() {
                fallback = Some(path);
            }
        }
    }

    fallback.ok_or_else(|| anyhow::anyhow!("no BlueZ adapter found (no org.bluez.Adapter1 objects)"))
}
