use std::{thread, time::Duration};
use esp_idf_svc::log::EspLogger;

mod fingerprint;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();

    log::info!("=== Test BM-Lite ===");

    fingerprint::init()?;
    // toujours enrôler 4 fois au démarrage (à chaque lancement)
    // ✅ Toujours enrôler 1 fois au démarrage (à chaque lancement)
    log::info!("On va enrôler un doigt (1 fois)...");
    fingerprint::wipe_templates()?;          // optionnel mais conseillé si tu veux repartir à zéro
    fingerprint::enroll_user()?;             // enrôlement une fois
    log::info!("✅ Enrôlement terminé");

    //boucle de vérification
    loop {
        log::info!("Pose ton doigt sur le capteur...");

        match fingerprint::check_once(5_000) {
            Ok(true) => log::info!("✅ Doigt reconnu"),
            Ok(false) => log::warn!("❌ Doigt non reconnu"),
            Err(e) => log::error!("Erreur BM-Lite: {e}"),
        } // toujours enroller 5 fois au démarrage

        thread::sleep(Duration::from_millis(500));
    }
}
