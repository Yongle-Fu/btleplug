use async_trait::async_trait;
use bluez_async::{
    BluetoothEvent, BluetoothSession, CharacteristicEvent, CharacteristicFlags, CharacteristicId,
    CharacteristicInfo, DescriptorInfo, DeviceId, DeviceInfo, MacAddress, ServiceInfo,
    WriteOptions,
};
use futures::future::{join_all, ready};
use futures::stream::{Stream, StreamExt};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "serde")]
use serde_cr as serde;
use std::collections::{BTreeSet, HashMap};
use std::fmt::{self, Display, Formatter};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::api::{
    self, AddressType, BDAddr, CharPropFlags, Characteristic, Descriptor, PeripheralProperties,
    Service, ValueNotification, WriteType,
};
use crate::{Error, Result};

#[derive(Clone, Debug)]
struct CharacteristicInternal {
    info: CharacteristicInfo,
    descriptors: HashMap<Uuid, DescriptorInfo>,
}

impl CharacteristicInternal {
    fn new(info: CharacteristicInfo, descriptors: HashMap<Uuid, DescriptorInfo>) -> Self {
        Self { info, descriptors }
    }
}

#[derive(Clone, Debug)]
struct ServiceInternal {
    info: ServiceInfo,
    characteristics: HashMap<Uuid, CharacteristicInternal>,
}

#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_cr")
)]
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PeripheralId(pub(crate) DeviceId);

impl PeripheralId {
    pub fn from_str(s: &str) -> Self {
        Self(DeviceId::new(s))
    }
}

impl Display for PeripheralId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Implementation of [api::Peripheral](crate::api::Peripheral).
#[derive(Clone, Debug)]
pub struct Peripheral {
    session: BluetoothSession,
    device: DeviceId,
    mac_address: BDAddr,
    services: Arc<Mutex<HashMap<Uuid, ServiceInternal>>>,
}

fn get_characteristic<'a>(
    services: &'a HashMap<Uuid, ServiceInternal>,
    service_uuid: &Uuid,
    characteristic_uuid: &Uuid,
) -> Result<&'a CharacteristicInternal> {
    services
        .get(service_uuid)
        .ok_or_else(|| {
            Error::Other(format!("Service with UUID {} not found.", service_uuid).into())
        })?
        .characteristics
        .get(characteristic_uuid)
        .ok_or_else(|| {
            Error::Other(
                format!(
                    "Characteristic with UUID {} not found.",
                    characteristic_uuid
                )
                .into(),
            )
        })
}

impl Peripheral {
    pub(crate) fn new(session: BluetoothSession, device: DeviceInfo) -> Self {
        Peripheral {
            session,
            device: device.id,
            mac_address: device.mac_address.into(),
            services: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn characteristic_info(&self, characteristic: &Characteristic) -> Result<CharacteristicInfo> {
        let services = self.services.lock().map_err(Into::<Error>::into)?;
        get_characteristic(
            &services,
            &characteristic.service_uuid,
            &characteristic.uuid,
        )
        .map(|c| &c.info)
        .cloned()
    }

    fn descriptor_info(&self, descriptor: &Descriptor) -> Result<DescriptorInfo> {
        let services = self.services.lock().map_err(Into::<Error>::into)?;
        let characteristic = get_characteristic(
            &services,
            &descriptor.service_uuid,
            &descriptor.characteristic_uuid,
        )?;
        characteristic
            .descriptors
            .get(&descriptor.uuid)
            .ok_or_else(|| {
                Error::Other(format!("Descriptor with UUID {} not found.", descriptor.uuid).into())
            })
            .cloned()
    }

    async fn device_info(&self) -> Result<DeviceInfo> {
        Ok(self.session.get_device_info(&self.device).await?)
    }
}

#[async_trait]
impl api::Peripheral for Peripheral {
    fn id(&self) -> PeripheralId {
        PeripheralId(self.device.to_owned())
    }

    fn address(&self) -> BDAddr {
        self.mac_address
    }

    async fn properties(&self) -> Result<Option<PeripheralProperties>> {
        let device_info = self.device_info().await?;
        Ok(Some(PeripheralProperties {
            address: device_info.mac_address.into(),
            address_type: Some(device_info.address_type.into()),
            local_name: device_info.name,
            tx_power_level: device_info.tx_power,
            rssi: device_info.rssi,
            manufacturer_data: device_info.manufacturer_data,
            service_data: device_info.service_data,
            services: device_info.services,
            class: device_info.class,
        }))
    }

    fn services(&self) -> BTreeSet<Service> {
        self.services
            .lock()
            .unwrap()
            .values()
            .map(|service| service.into())
            .collect()
    }

    async fn is_connected(&self) -> Result<bool> {
        let device_info = self.device_info().await?;
        Ok(device_info.connected)
    }

    async fn connect(&self) -> Result<()> {
        self.session.connect(&self.device).await?;
        Ok(())
    }

    async fn disconnect(&self) -> Result<()> {
        self.session.disconnect(&self.device).await?;
        Ok(())
    }

    async fn discover_services(&self) -> Result<()> {
        let mut services_internal = HashMap::new();
        let services = self.session.get_services(&self.device).await?;
        for service in services {
            let characteristics = self.session.get_characteristics(&service.id).await?;
            let characteristics = join_all(
                characteristics
                    .into_iter()
                    .fold(
                        // Only consider the first characteristic of each UUID
                        // This "should" be unique, but of course it's not enforced
                        HashMap::<Uuid, CharacteristicInfo>::new(),
                        |mut map, characteristic| {
                            if !map.contains_key(&characteristic.uuid) {
                                map.insert(characteristic.uuid, characteristic);
                            }
                            map
                        },
                    )
                    .into_iter()
                    .map(|mapped_characteristic| async {
                        let characteristic = mapped_characteristic.1;
                        let descriptors = self
                            .session
                            .get_descriptors(&characteristic.id)
                            .await
                            .unwrap_or(Vec::new())
                            .into_iter()
                            .map(|descriptor| (descriptor.uuid, descriptor))
                            .collect();
                        CharacteristicInternal::new(characteristic, descriptors)
                    }),
            )
            .await;
            services_internal.insert(
                service.uuid,
                ServiceInternal {
                    info: service,
                    characteristics: characteristics
                        .into_iter()
                        .map(|characteristic| (characteristic.info.uuid, characteristic))
                        .collect(),
                },
            );
        }
        *(self.services.lock().map_err(Into::<Error>::into)?) = services_internal;
        Ok(())
    }

    async fn write(
        &self,
        characteristic: &Characteristic,
        data: &[u8],
        write_type: WriteType,
    ) -> Result<()> {
        let characteristic_info = self.characteristic_info(characteristic)?;
        let options = WriteOptions {
            write_type: Some(write_type.into()),
            ..Default::default()
        };
        Ok(self
            .session
            .write_characteristic_value_with_options(&characteristic_info.id, data, options)
            .await?)
    }

    async fn read(&self, characteristic: &Characteristic) -> Result<Vec<u8>> {
        let characteristic_info = self.characteristic_info(characteristic)?;
        Ok(self
            .session
            .read_characteristic_value(&characteristic_info.id)
            .await?)
    }

    async fn subscribe(&self, characteristic: &Characteristic) -> Result<()> {
        let characteristic_info = self.characteristic_info(characteristic)?;
        Ok(self.session.start_notify(&characteristic_info.id).await?)
    }

    async fn unsubscribe(&self, characteristic: &Characteristic) -> Result<()> {
        let characteristic_info = self.characteristic_info(characteristic)?;
        Ok(self.session.stop_notify(&characteristic_info.id).await?)
    }

    async fn notifications(&self) -> Result<Pin<Box<dyn Stream<Item = ValueNotification> + Send>>> {
        let device_id = self.device.clone();
        let events = self.session.device_event_stream(&device_id).await?;
        let services = self.services.clone();
        Ok(Box::pin(events.filter_map(move |event| {
            ready(value_notification(event, &device_id, services.clone()))
        })))
    }

    async fn write_descriptor(&self, descriptor: &Descriptor, data: &[u8]) -> Result<()> {
        let descriptor_info = self.descriptor_info(descriptor)?;
        Ok(self
            .session
            .write_descriptor_value(&descriptor_info.id, data)
            .await?)
    }

    async fn read_descriptor(&self, descriptor: &Descriptor) -> Result<Vec<u8>> {
        let descriptor_info = self.descriptor_info(descriptor)?;
        Ok(self
            .session
            .read_descriptor_value(&descriptor_info.id)
            .await?)
    }
}

fn value_notification(
    event: BluetoothEvent,
    device_id: &DeviceId,
    services: Arc<Mutex<HashMap<Uuid, ServiceInternal>>>,
) -> Option<ValueNotification> {
    match event {
        BluetoothEvent::Characteristic {
            id,
            event: CharacteristicEvent::Value { value },
        } if id.service().device() == *device_id => {
            let services = services.lock().unwrap();
            let uuid = find_characteristic_by_id(&services, id)?.uuid;
            Some(ValueNotification { uuid, value })
        }
        _ => None,
    }
}

fn find_characteristic_by_id(
    services: &HashMap<Uuid, ServiceInternal>,
    characteristic_id: CharacteristicId,
) -> Option<&CharacteristicInfo> {
    for service in services.values() {
        for characteristic in service.characteristics.values() {
            if characteristic.info.id == characteristic_id {
                return Some(&characteristic.info);
            }
        }
    }
    None
}

impl From<WriteType> for bluez_async::WriteType {
    fn from(write_type: WriteType) -> Self {
        match write_type {
            WriteType::WithoutResponse => bluez_async::WriteType::WithoutResponse,
            WriteType::WithResponse => bluez_async::WriteType::WithResponse,
        }
    }
}

impl From<MacAddress> for BDAddr {
    fn from(mac_address: MacAddress) -> Self {
        <[u8; 6]>::into(mac_address.into())
    }
}

impl From<DeviceId> for PeripheralId {
    fn from(device_id: DeviceId) -> Self {
        PeripheralId(device_id)
    }
}

impl From<bluez_async::AddressType> for AddressType {
    fn from(address_type: bluez_async::AddressType) -> Self {
        match address_type {
            bluez_async::AddressType::Public => AddressType::Public,
            bluez_async::AddressType::Random => AddressType::Random,
        }
    }
}

fn make_descriptor(
    info: &DescriptorInfo,
    characteristic_uuid: Uuid,
    service_uuid: Uuid,
) -> Descriptor {
    Descriptor {
        uuid: info.uuid,
        characteristic_uuid,
        service_uuid,
    }
}

fn make_characteristic(
    characteristic: &CharacteristicInternal,
    service_uuid: Uuid,
) -> Characteristic {
    let CharacteristicInternal { info, descriptors } = characteristic;
    Characteristic {
        uuid: info.uuid,
        properties: info.flags.into(),
        descriptors: descriptors
            .iter()
            .map(|(_, descriptor)| make_descriptor(descriptor, info.uuid, service_uuid))
            .collect(),
        service_uuid,
    }
}

impl From<&ServiceInternal> for Service {
    fn from(service: &ServiceInternal) -> Self {
        Service {
            uuid: service.info.uuid,
            primary: service.info.primary,
            characteristics: service
                .characteristics
                .values()
                .map(|characteristic| make_characteristic(characteristic, service.info.uuid))
                .collect(),
        }
    }
}

impl From<CharacteristicFlags> for CharPropFlags {
    fn from(flags: CharacteristicFlags) -> Self {
        let mut result = CharPropFlags::default();
        if flags.contains(CharacteristicFlags::BROADCAST) {
            result.insert(CharPropFlags::BROADCAST);
        }
        if flags.contains(CharacteristicFlags::READ) {
            result.insert(CharPropFlags::READ);
        }
        if flags.contains(CharacteristicFlags::WRITE_WITHOUT_RESPONSE) {
            result.insert(CharPropFlags::WRITE_WITHOUT_RESPONSE);
        }
        if flags.contains(CharacteristicFlags::WRITE) {
            result.insert(CharPropFlags::WRITE);
        }
        if flags.contains(CharacteristicFlags::NOTIFY) {
            result.insert(CharPropFlags::NOTIFY);
        }
        if flags.contains(CharacteristicFlags::INDICATE) {
            result.insert(CharPropFlags::INDICATE);
        }
        if flags.contains(CharacteristicFlags::SIGNED_WRITE) {
            result.insert(CharPropFlags::AUTHENTICATED_SIGNED_WRITES);
        }
        if flags.contains(CharacteristicFlags::EXTENDED_PROPERTIES) {
            result.insert(CharPropFlags::EXTENDED_PROPERTIES);
        }
        result
    }
}
