//! TL360 backend. El binario (`main.rs`) solo arranca el servidor; todo lo demás vive acá para
//! poder probarlo desde `tests/` sin levantar un puerto.
pub mod agenda;
pub mod auth_extractor;
pub mod errors;
pub mod handlers;
pub mod mailer;
pub mod push;
pub mod rate_limit;
pub mod recordatorios;
pub mod resumen_semanal;
pub mod routes;
pub mod state;
pub mod validation;
