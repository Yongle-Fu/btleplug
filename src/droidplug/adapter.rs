use super::jni_utils::exceptions::try_block;
use super::{
    jni::{
        global_jvm,
        objects::{JScanFilter, JScanResult},
    },
    peripheral::{Peripheral, PeripheralId},
};
use crate::{
    Error, Result,
    api::{
        BDAddr, Central, CentralEvent, CentralState, GattTransport, PeripheralProperties,
        ScanFilter,
    },
    common::adapter_manager::AdapterManager,
};
use async_trait::async_trait;
use futures::stream::Stream;
use jni::objects::JClass;
use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JString},
    strings::JavaStr,
    sys::{jboolean, jobjectArray},
};
use std::{
    fmt::{Debug, Formatter},
    pin::Pin,
    str::FromStr,
    sync::Arc,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Adapter {
    manager: Arc<AdapterManager<Peripheral>>,
    internal: GlobalRef,
}

impl Debug for Adapter {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.debug_struct("Adapter")
            .field("manager", &self.manager)
            .finish()
    }
}

impl Adapter {
    pub(crate) fn new() -> Result<Self> {
        let env = global_jvm().get_env()?;

        let obj = env.new_object(
            "com/nonpolynomial/btleplug/android/impl/Adapter",
            "()V",
            &[],
        )?;
        let internal = env.new_global_ref(obj)?;
        let adapter = Self {
            manager: Arc::new(AdapterManager::default()),
            internal,
        };
        env.set_rust_field(obj, "handle", adapter.clone())?;

        Ok(adapter)
    }

    pub fn report_scan_result(&self, scan_result: JObject) -> Result<Peripheral> {
        use std::convert::TryInto;

        let env = global_jvm().get_env()?;
        let scan_result = JScanResult::from_env(&env, scan_result)?;

        let (addr, properties): (BDAddr, Option<PeripheralProperties>) = scan_result.try_into()?;

        match self.manager.peripheral(&PeripheralId(addr)) {
            Some(p) => match properties {
                Some(properties) => {
                    self.report_properties(&p, properties, false);
                    Ok(p)
                }
                None => {
                    //self.manager.emit(CentralEvent::DeviceDisconnected(addr));
                    Err(Error::DeviceNotFound)
                }
            },
            None => match properties {
                Some(properties) => {
                    let p = self.add(addr)?;
                    self.report_properties(&p, properties, true);
                    Ok(p)
                }
                None => Err(Error::DeviceNotFound),
            },
        }
    }

    pub fn report_bonded_device(
        &self,
        address: BDAddr,
        name: Option<String>,
        services: Vec<Uuid>,
    ) -> Result<Peripheral> {
        let properties = PeripheralProperties {
            address,
            local_name: name.clone(),
            advertisement_name: name,
            services,
            ..PeripheralProperties::default()
        };
        match self.manager.peripheral(&PeripheralId(address)) {
            Some(peripheral) => {
                self.report_properties(&peripheral, properties, false);
                Ok(peripheral)
            }
            None => {
                let peripheral = self.add(address)?;
                self.report_properties(&peripheral, properties, true);
                Ok(peripheral)
            }
        }
    }

    fn add(&self, address: BDAddr) -> Result<Peripheral> {
        let env = global_jvm().get_env()?;
        let peripheral = Peripheral::new(&env, self.internal.as_obj(), address)?;
        self.manager.add_peripheral(peripheral.clone());
        Ok(peripheral)
    }

    fn report_properties(
        &self,
        peripheral: &Peripheral,
        properties: PeripheralProperties,
        new: bool,
    ) {
        peripheral.report_properties(properties.clone());
        self.manager.emit(if new {
            CentralEvent::DeviceDiscovered(PeripheralId(properties.address))
        } else {
            CentralEvent::DeviceUpdated(PeripheralId(properties.address))
        });
        self.manager
            .emit(CentralEvent::ManufacturerDataAdvertisement {
                id: PeripheralId(properties.address),
                manufacturer_data: properties.manufacturer_data,
            });
        self.manager.emit(CentralEvent::ServiceDataAdvertisement {
            id: PeripheralId(properties.address),
            service_data: properties.service_data,
        });
        self.manager.emit(CentralEvent::ServicesAdvertisement {
            id: PeripheralId(properties.address),
            services: properties.services,
        });
    }
}

#[async_trait]
impl Central for Adapter {
    type Peripheral = Peripheral;

    async fn adapter_info(&self) -> Result<String> {
        // TODO: Get information about the adapter.
        Ok("Android".to_string())
    }

    async fn events(&self) -> Result<Pin<Box<dyn Stream<Item = CentralEvent> + Send>>> {
        Ok(self.manager.event_stream())
    }

    async fn start_scan(&self, filter: ScanFilter) -> Result<()> {
        let env = global_jvm().get_env()?;
        let filter = JScanFilter::new(&env, filter)?;
        try_block(&env, || {
            env.call_method(
                &self.internal,
                "startScan",
                "(Lcom/nonpolynomial/btleplug/android/impl/ScanFilter;)V",
                &[filter.into()],
            )?;
            Ok(Ok(()))
        })
        .catch(
            JClass::from(
                super::jni_utils::classcache::get_class(
                    "com/nonpolynomial/btleplug/android/impl/NoBluetoothAdapterException",
                )
                .unwrap()
                .as_obj(),
            ),
            |_ex| Ok(Err(Error::NoAdapterAvailable)),
        )
        .catch("java/lang/RuntimeException", |ex| {
            let msg = env
                .call_method(ex, "getMessage", "()Ljava/lang/String;", &[])?
                .l()?;
            let msgstr: String = env.get_string(msg.into())?.into();
            Ok(Err(Error::RuntimeError(msgstr)))
        })
        .result()?
    }

    async fn start_scan_with_transport(
        &self,
        filter: ScanFilter,
        transport: GattTransport,
    ) -> Result<()> {
        let env = global_jvm().get_env()?;
        let filter = JScanFilter::new(&env, filter)?;
        // android.bluetooth.BluetoothDevice transport constants:
        // AUTO=0, BREDR=1, LE=2.
        let transport = match transport {
            GattTransport::Auto => 0,
            GattTransport::BrEdr => 1,
            GattTransport::Le => 2,
        };
        try_block(&env, || {
            env.call_method(
                &self.internal,
                "startScanWithTransport",
                "(Lcom/nonpolynomial/btleplug/android/impl/ScanFilter;I)V",
                &[filter.into(), transport.into()],
            )?;
            Ok(Ok(()))
        })
        .catch(
            JClass::from(
                super::jni_utils::classcache::get_class(
                    "com/nonpolynomial/btleplug/android/impl/NoBluetoothAdapterException",
                )
                .unwrap()
                .as_obj(),
            ),
            |_ex| Ok(Err(Error::NoAdapterAvailable)),
        )
        .catch("java/lang/RuntimeException", |ex| {
            let msg = env
                .call_method(ex, "getMessage", "()Ljava/lang/String;", &[])?
                .l()?;
            let msgstr: String = env.get_string(msg.into())?.into();
            Ok(Err(Error::RuntimeError(msgstr)))
        })
        .result()?
    }

    async fn stop_scan(&self) -> Result<()> {
        let env = global_jvm().get_env()?;
        env.call_method(&self.internal, "stopScan", "()V", &[])?;
        Ok(())
    }

    async fn peripherals(&self) -> Result<Vec<Peripheral>> {
        Ok(self.manager.peripherals())
    }

    async fn peripheral(&self, address: &PeripheralId) -> Result<Peripheral> {
        self.manager
            .peripheral(address)
            .ok_or(Error::DeviceNotFound)
    }

    async fn add_peripheral(&self, address: &PeripheralId) -> Result<Peripheral> {
        self.add(address.0)
    }

    async fn clear_peripherals(&self) -> Result<()> {
        self.manager.clear_peripherals();
        Ok(())
    }

    async fn adapter_state(&self) -> Result<CentralState> {
        Ok(CentralState::Unknown)
    }
}

pub(crate) fn adapter_report_scan_result_internal(
    env: &JNIEnv,
    obj: JObject,
    scan_result: JObject,
) -> crate::Result<()> {
    let adapter = env.get_rust_field::<_, _, Adapter>(obj, "handle")?;
    adapter.report_scan_result(scan_result)?;
    Ok(())
}

pub(crate) fn adapter_report_bonded_device_internal(
    env: &JNIEnv,
    obj: JObject,
    address: JString,
    name: JString,
    service_uuids: JObject,
) -> crate::Result<()> {
    let adapter = env.get_rust_field::<_, _, Adapter>(obj, "handle")?;
    let address = JavaStr::from_env(env, address)?;
    let address = BDAddr::from_str(
        address
            .to_str()
            .map_err(|error| Error::Other(error.into()))?,
    )?;
    let name = JavaStr::from_env(env, name)?;
    let name = name
        .to_str()
        .map_err(|error| Error::Other(error.into()))?
        .to_string();
    let name = (!name.is_empty()).then_some(name);

    let array = service_uuids.into_inner() as jobjectArray;
    let mut services = Vec::with_capacity(env.get_array_length(array)? as usize);
    for index in 0..env.get_array_length(array)? {
        let value = env.get_object_array_element(array, index)?;
        let value = JavaStr::from_env(env, JString::from(value))?;
        let value = value.to_str().map_err(|error| Error::Other(error.into()))?;
        services.push(Uuid::parse_str(value).map_err(|error| Error::Other(error.into()))?);
    }
    adapter.report_bonded_device(address, name, services)?;
    Ok(())
}

pub(crate) fn adapter_on_connection_state_changed_internal(
    env: &JNIEnv,
    obj: JObject,
    addr: JString,
    connected: jboolean,
) -> crate::Result<()> {
    let adapter = env.get_rust_field::<_, _, Adapter>(obj, "handle")?;
    let addr_str = JavaStr::from_env(env, addr)?;
    let addr_str = addr_str.to_str().map_err(|e| Error::Other(e.into()))?;
    let addr = BDAddr::from_str(addr_str)?;
    adapter.manager.emit(if connected != 0 {
        CentralEvent::DeviceConnected(PeripheralId(addr))
    } else {
        CentralEvent::DeviceDisconnected(PeripheralId(addr))
    });
    Ok(())
}
