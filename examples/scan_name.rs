//! Usage: cargo run --example scan_name
//!
//! This example performs a BLE scan *without* applying a Service UUID filter.
//! It is used to prove that Windows natively receives and processes Scan Responses
//! (which contain the `COMPLETE_LOCAL_NAME`) dynamically. If a UUID filter is applied,
//! Windows may drop the Scan Response packets silently.
use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use futures::stream::StreamExt;
use std::error::Error;
use std::time::Duration;
use tokio::time;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters.into_iter().nth(0).expect("No adapter found");

    let use_filter = std::env::args().any(|arg| arg == "--with-uuid");

    let filter = if use_filter {
        println!("Scanning WITH UUID filter (to demonstrate Windows drop bug) ...");
        let brsc_uuid = uuid::Uuid::parse_str("4de5a20c-0001-ae14-bf63-0242ac130002").unwrap();
        ScanFilter {
            services: vec![brsc_uuid],
        }
    } else {
        println!("Scanning WITHOUT UUID filter (Scan Responses should pass normally) ...");
        ScanFilter::default()
    };
    
    central.start_scan(filter).await?;

    let mut events = central.events().await?;

    // Keep scanning for 10 seconds
    let timeout = time::sleep(Duration::from_secs(10));
    tokio::pin!(timeout);

    println!("Listening for device updates...");
    loop {
        tokio::select! {
            Some(event) = events.next() => {
                match event {
                    CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => {
                        if let Ok(peripheral) = central.peripheral(&id).await {
                            if let Ok(Some(props)) = peripheral.properties().await {
                                let name = props.local_name.unwrap_or_else(|| "N/A".to_string());
                                // We filter console prints locally to just BrainSync or ones with actual names
                                if name != "N/A" {
                                    println!("Found/Updated: Device: {} | Address: {} | RSSI: {:?}", name, id, props.rssi);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = &mut timeout => {
                break;
            }
        }
    }

    println!("Scan finished.");
    Ok(())
}
