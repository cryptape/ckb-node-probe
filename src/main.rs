pub use config::{init_config, Config};
use crossbeam::channel::bounded;
use influxdb::{Client, ReadQuery};
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::env::var;
use std::process::exit;

mod app_config;
mod chain;
mod config;
mod get_version;
mod network;
mod topology;

lazy_static! {
    static ref LOG_LEVEL: String = var("LOG_LEVEL").unwrap_or_else(|_| "ERROR".to_string());
    static ref CONFIG: Config = {
        let config_path = var("CKB_ANALYZER_CONFIG").unwrap_or_else(|_| {
            panic!("please specify config path via environment variable CKB_ANALYZER_CONFIG")
        });
        init_config(config_path)
    };
    static ref HOSTNAME: String = var("HOSTNAME")
        .unwrap_or_else(|_| gethostname::gethostname().to_string_lossy().to_string());
    static ref INFLUXDB_USERNAME: String =
        var("INFLUXDB_USERNAME").unwrap_or_else(|_| "".to_string());
    static ref INFLUXDB_PASSWORD: String =
        var("INFLUXDB_PASSWORD").unwrap_or_else(|_| "".to_string());
}

#[tokio::main]
async fn main() {
    let influx = if INFLUXDB_USERNAME.is_empty() {
        Client::new(
            CONFIG.influxdb.url.as_str(),
            CONFIG.influxdb.database.as_str(),
        )
    } else {
        Client::new(
            CONFIG.influxdb.url.as_str(),
            CONFIG.influxdb.database.as_str(),
        )
        .with_auth(INFLUXDB_USERNAME.as_str(), INFLUXDB_PASSWORD.as_str())
    };
    let (query_sender, query_receiver) = bounded(5000);

    assert!(CONFIG.network.enabled || CONFIG.chain.enabled || CONFIG.topology.enabled);
    if CONFIG.network.enabled {
        network::spawn_analyze(query_sender.clone());
    }
    if CONFIG.chain.enabled {
        let sql = format!(
            "SELECT last(number) FROM blocks WHERE network = '{}'",
            CONFIG.network.ckb_network_name
        );
        let query_last_number = ReadQuery::new(&sql);
        let last_number = match influx.query(&query_last_number).await {
            Err(err) => {
                eprintln!("influxdb.query(\"{}\"), error: {}", sql, err);
                exit(1);
            }
            Ok(results) => {
                let json: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&results).unwrap();
                let results = json.get("results").unwrap().as_array().unwrap();
                let result = results.get(0).unwrap().as_object().unwrap();
                if let Some(series) = result.get("series") {
                    let series = series.as_array().unwrap();
                    let serie = series.get(0).unwrap().as_object().unwrap();
                    let values = serie.get("values").unwrap().as_array().unwrap();
                    let value = values.get(0).unwrap().as_array().unwrap();
                    value.get(1).unwrap().as_u64().unwrap()
                } else {
                    1
                }
            }
        };

        chain::spawn_analyze(query_sender.clone(), last_number);
    }
    if CONFIG.topology.enabled {
        topology::spawn_analyze(query_sender);
    }

    for mut query in query_receiver {
        // Attach built-in tags
        query = query
            .add_tag("network", CONFIG.network.ckb_network_name.clone())
            .add_tag("hostname", HOSTNAME.clone());

        // Writes asynchronously
        let influx_ = influx.clone();
        tokio::spawn(async move { influx_.query(&query).await });

        // Writes synchronously
        // let write_result = influx.query(&query).await;
        // if let Err(err) = write_result {
        //     eprintln!("influxdb.query, error: {}", err);
        // }
    }
}
