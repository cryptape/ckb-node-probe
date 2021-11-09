use crate::tokio;
use std::env;
use std::time::Duration;

/// PeerScanner scan the null-country entries in `peer` table and write back
/// country shortcuts back to database.
pub struct PeerScanner {
    pg: tokio_postgres::Client,
    ipinfo: ipinfo::IpInfo,
    network_id: String,
}

impl PeerScanner {
    pub async fn new(pg_config: &tokio_postgres::Config, network_id: String) -> Self {
        let (pg, pg_connection) = pg_config.connect(tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(async move {
            if let Err(err) = pg_connection.await {
                log::error!("postgres connection error: {:?}", err);
            }
        });

        let ipinfo_io_token = match env::var("IPINFO_IO_TOKEN") {
            Ok(token) if !token.is_empty() => Some(token),
            _ => {
                log::warn!("miss environment variable \"IPINFO_IO_TOKEN\", use empty value");
                None
            }
        };
        let ipinfo = ipinfo::IpInfo::new(ipinfo::IpInfoConfig {
            token: ipinfo_io_token,
            cache_size: 1000,
            timeout: ::std::time::Duration::from_secs(2 * 60),
        })
        .expect("connect to https://ipinfo.io");

        Self {
            pg,
            ipinfo,
            network_id,
        }
    }

    pub async fn run(&mut self) {
        loop {
            if let Err(err) = self.run_().await {
                log::error!("postgres error: {:?}", err);
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }

    async fn run_(&mut self) -> Result<(), tokio_postgres::Error> {
        let statement = self
            .pg
            .prepare(&format!("SELECT id, time, version, ip FROM {}.peer WHERE id > $1 AND country IS NULL ORDER BY ID LIMIT 100", self.network_id))
            .await?;
        let mut last_id = 0i32;

        loop {
            let raws = self.pg.query(&statement, &[&last_id]).await?;
            if raws.is_empty() {
                log::debug!("select empty null-country peer entries");
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }

            last_id = raws[raws.len() - 1].get(0);
            log::debug!(
                "select {} null-country peer entries, last id is {}",
                raws.len(),
                last_id
            );

            for raw in raws {
                let id: i32 = raw.get(0);
                let ip: String = raw.get(3);

                match self.lookup_country(&ip) {
                    Ok(country) => {
                        let raw_query = format!(
                            "UPDATE {}.peer SET country = '{}' WHERE id = {}",
                            self.network_id, country, id,
                        );
                        self.pg.batch_execute(&raw_query).await?;
                    }
                    Err(err) => {
                        log::error!("ipinfo.io query error: {:?}", err);
                        continue;
                    }
                }
            }
        }
    }

    pub fn lookup_country(&mut self, ip: &str) -> Result<String, ipinfo::IpError> {
        let info_map = self.ipinfo.lookup(&[ip])?;
        let ipdetail = info_map[ip].to_owned();
        Ok(ipdetail.country)
    }
}