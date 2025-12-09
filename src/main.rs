use std::{thread, time::Duration};
use esp_idf_svc::log::EspLogger;

mod fingerprint;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();

    log::info!("=== Test minimal BM-Lite ===");

    fingerprint::init()?;
    //fingerprint::wipe_templates()?;
    fingerprint::enroll_user_if_needed()?;

    loop {
        log::info!("Pose ton doigt sur le capteur...");

        match fingerprint::check_once(3_000) {
            Ok(true) => log::info!("✅ Doigt reconnu"),
            Ok(false) => log::warn!("❌ Doigt non reconnu"),
            Err(e) => log::error!("Erreur BM-Lite: {e}"),
        }

        thread::sleep(Duration::from_secs(2));
    }
}
