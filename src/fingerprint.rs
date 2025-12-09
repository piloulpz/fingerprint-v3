use anyhow::{anyhow, Result};
use core::ptr;
use lazy_static::lazy_static;
use std::sync::Mutex;

use esp_idf_svc::sys::bmlite::{
    console_initparams_t,
    gpio_num_t_GPIO_NUM_16,
    gpio_num_t_GPIO_NUM_35,
    gpio_num_t_GPIO_NUM_36,
    gpio_num_t_GPIO_NUM_37,
    gpio_num_t_GPIO_NUM_45,
    gpio_num_t_GPIO_NUM_48,
    interface_t_SPI_INTERFACE,
    pin_config_t,
    spi_host_device_t_SPI2_HOST,
    HCP_arg_t,
    HCP_comm_t,
    MTU,
    // Résultats / status
    fpc_bep_result_t_FPC_BEP_RESULT_OK,
    // Fonctions haut niveau BM-Lite
    bep_enroll_finger,
    bep_identify_finger,
    bep_sensor_calibrate,
    bep_sw_reset,
    bep_template_get_count,
    bep_template_remove_all,
    bep_template_save,
    // Init plate-forme (SPI + GPIO + reset capteur)
    platform_deinit,
    platform_init,
};

/// Contexte minimal du capteur
struct SensorCtx {
    params: *mut console_initparams_t,
    pins: *mut pin_config_t,
    chain: *mut HCP_comm_t,
    initialized: bool,
}

// On garantit au compilateur que ce type peut être partagé/envoyé entre threads.
// Sur ESP32 avec notre usage contrôlé (tout passe par le Mutex), c’est ok.
unsafe impl Send for SensorCtx {}
unsafe impl Sync for SensorCtx {}

impl SensorCtx {
    const fn new() -> Self {
        Self {
            params: ptr::null_mut(),
            pins: ptr::null_mut(),
            chain: ptr::null_mut(),
            initialized: false,
        }
    }

    fn set(&mut self, params: *mut console_initparams_t, pins: *mut pin_config_t, chain: *mut HCP_comm_t) {
        self.params = params;
        self.pins = pins;
        self.chain = chain;
        self.initialized = true;
    }

    fn reset(&mut self) {
        self.params = ptr::null_mut();
        self.pins = ptr::null_mut();
        self.chain = ptr::null_mut();
        self.initialized = false;
    }

    fn is_set(&self) -> bool {
        self.initialized && !self.chain.is_null()
    }
}

lazy_static! {
    static ref SENSOR_CTX: Mutex<SensorCtx> = Mutex::new(SensorCtx::new());
}

/// Helper pour checker les codes de retour C
fn check_bep(res: i32, what: &str) -> Result<()> {
    if res == fpc_bep_result_t_FPC_BEP_RESULT_OK {
        Ok(())
    } else {
        Err(anyhow!("{what} failed with code {res}"))
    }
}

/// Alloue et configure les structs C : HCP_comm_t, pin_config_t, console_initparams_t.
///
/// Les pins / SPI sont ceux que tu utilisais déjà :
/// - SPI2
/// - CS   = GPIO45
/// - MISO = GPIO37
/// - MOSI = GPIO35
/// - CLK  = GPIO36
/// - RST  = GPIO48 (active bas)
/// - IRQ  = GPIO16 (active haut)
unsafe fn alloc_config() -> Result<(*mut console_initparams_t, *mut pin_config_t, *mut HCP_comm_t)> {
    // Buffers pour la couche HCP
    let pkt_buffer = Box::into_raw(Box::new([0u8; 1024 * 3])) as *mut u8;
    let txrx_buffer = Box::into_raw(Box::new([0u8; MTU as usize])) as *mut u8;

    let chain = Box::into_raw(Box::new(HCP_comm_t {
        write: None,
        read: None,
        phy_rx_timeout: 2000,
        pkt_buffer,
        pkt_size: 0,
        pkt_size_max: 1024 * 3,
        txrx_buffer,
        arg: HCP_arg_t::default(),
        bep_result: 0,
    }));

    let pins = Box::into_raw(Box::new(pin_config_t {
        spi_host: spi_host_device_t_SPI2_HOST,
        cs_n_pin: gpio_num_t_GPIO_NUM_45,
        miso_pin: gpio_num_t_GPIO_NUM_37,
        rst_pin: gpio_num_t_GPIO_NUM_48,
        mosi_pin: gpio_num_t_GPIO_NUM_35,
        irq_pin: gpio_num_t_GPIO_NUM_16,
        spi_clk_pin: gpio_num_t_GPIO_NUM_36,
    }));

    let params = Box::into_raw(Box::new(console_initparams_t {
        iface: interface_t_SPI_INTERFACE,
        port: ptr::null_mut(),
        baudrate: 5_000_000,
        timeout: 3000,
        pins,
        hcp_comm: chain,
    }));

    Ok((params, pins, chain))
}

/// Initialisation minimale du BM-Lite :
/// - configure SPI + GPIO via `platform_init`
/// - fait un reset du capteur dans `platform_init` / `platform_bmlite_reset`
pub fn init() -> Result<()> {
    let mut ctx = SENSOR_CTX.lock().unwrap();

    if ctx.is_set() {
        return Ok(());
    }

    unsafe {
        let (params, pins, chain) = alloc_config()?;

        // platform_init(void *params) -> fpc_bep_result_t
        let res = platform_init(params.cast());
        check_bep(res, "platform_init")?;

        ctx.set(params, pins, chain);
    }

    log::info!("BM-Lite: init OK");
    Ok(())
}

/// Vérifie si exactement un template est stocké dans le capteur (ID peu importe).
pub fn is_user_enrolled() -> Result<bool> {
    let ctx = SENSOR_CTX.lock().unwrap();
    if !ctx.is_set() {
        return Err(anyhow!("BM-Lite not initialized"));
    }

    let mut count: u16 = 0;
    let res = unsafe { bep_template_get_count(ctx.chain, &mut count) };
    check_bep(res, "bep_template_get_count")?;

    Ok(count == 1)
}

/// Efface tous les templates stockés dans le BM-Lite.
pub fn wipe_templates() -> Result<()> {
    let ctx = SENSOR_CTX.lock().unwrap();
    if !ctx.is_set() {
        return Ok(());
    }

    let res = unsafe { bep_template_remove_all(ctx.chain) };
    check_bep(res, "bep_template_remove_all")?;

    Ok(())
}

/// Enrôle un utilisateur si aucun n’est présent.
/// - Calibrage
/// - Reset logiciel
/// - Enrôlement (bep_enroll_finger gère les captures / prompts)
/// - Sauvegarde en template ID = 1
pub fn enroll_user_if_needed() -> Result<()> {
    let mut ctx = SENSOR_CTX.lock().unwrap();

    if !ctx.is_set() {
        drop(ctx); // libère le lock
        init()?;
        ctx = SENSOR_CTX.lock().unwrap();
    }

    // Vérifie s'il y a déjà un template
    let mut count: u16 = 0;
    let res = unsafe { bep_template_get_count(ctx.chain, &mut count) };
    check_bep(res, "bep_template_get_count")?;

    if count > 0 {
        log::warn!("BM-Lite: un template existe déjà (count = {count}), on n'enrôle pas.");
        return Ok(());
    }

    log::info!("BM-Lite: calibration capteur...");
    let res = unsafe { bep_sensor_calibrate(ctx.chain) };
    check_bep(res, "bep_sensor_calibrate")?;

    log::info!("BM-Lite: reset logiciel...");
    let res = unsafe { bep_sw_reset(ctx.chain) };
    check_bep(res, "bep_sw_reset")?;

    log::info!("BM-Lite: enrôlement, pose ton doigt plusieurs fois...");
    let res = unsafe { bep_enroll_finger(ctx.chain) };
    check_bep(res, "bep_enroll_finger")?;

    // On sauvegarde sous l’ID = 1
    let template_id: u16 = 1;
    let res = unsafe { bep_template_save(ctx.chain, template_id) };
    check_bep(res, "bep_template_save")?;

    log::info!("BM-Lite: enrôlement terminé, template ID = {template_id}");

    Ok(())
}

/// Un seul test de doigt :
/// - timeout en ms
/// - retourne Ok(true) si le doigt correspond à un template (ID peu importe)
/// - Ok(false) si "pas de match"
/// - Err(..) si erreur de com / capteur
pub fn check_once(timeout_ms: u32) -> Result<bool> {
    let ctx = SENSOR_CTX.lock().unwrap();
    if !ctx.is_set() {
        return Err(anyhow!("BM-Lite not initialized"));
    }

    let mut template_id: u16 = 0;
    let mut matched: bool = false;

    // C côté bmlite_if: capture + extract + identify + ARG_MATCH/ARG_ID
    let res = unsafe { bep_identify_finger(ctx.chain, timeout_ms, &mut template_id, &mut matched) };
    check_bep(res, "bep_identify_finger")?;

    if matched {
        log::info!("BM-Lite: doigt reconnu, template ID = {template_id}");
    } else {
        log::info!("BM-Lite: doigt NON reconnu");
    }

    Ok(matched)
}

/// Optionnel : deinit propre si tu veux arrêter le capteur
pub fn deinit() -> Result<()> {
    let mut ctx = SENSOR_CTX.lock().unwrap();

    if !ctx.is_set() {
        return Ok(());
    }

    unsafe {
        let res = platform_deinit(ctx.params.cast());
        check_bep(res, "platform_deinit")?;
    }

    ctx.reset();
    log::info!("BM-Lite: deinit OK");

    Ok(())
}
