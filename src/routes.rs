use axum::{
    routing::{delete, get, post, put},
    Router,
};

use crate::handlers::{
    atenciones, auth, categorias, empresas, favoritos, health, moderacion, publicidades, push,
    solicitudes, turnos, usuarios,
};
use crate::state::AppState;

pub fn build(state: AppState) -> Router {
    let api = Router::new()
        .nest(
            "/auth",
            Router::new()
                .route("/register", post(auth::register))
                .route("/login", post(auth::login))
                .route("/verificar-email", post(auth::verificar_email))
                .route("/reenviar-verificacion", post(auth::reenviar_verificacion))
                .route("/forgot-password", post(auth::forgot_password))
                .route("/reset-password", post(auth::reset_password)),
        )
        .nest(
            "/usuarios",
            Router::new()
                .route(
                    "/",
                    get(usuarios::get_usuarios).post(usuarios::crear_usuario),
                )
                .route(
                    "/yo/recordatorio",
                    get(usuarios::get_recordatorio).put(usuarios::set_recordatorio),
                ),
        )
        .nest(
            "/categorias",
            Router::new()
                .route(
                    "/",
                    get(categorias::get_categorias).post(categorias::crear_categoria),
                )
                .route(
                    "/:id",
                    put(categorias::actualizar_categoria).delete(categorias::eliminar_categoria),
                ),
        )
        .nest(
            "/empresas",
            Router::new()
                .route(
                    "/",
                    get(empresas::get_empresas).post(empresas::create_empresa),
                )
                .route("/mapa", get(empresas::get_mapa))
                .route(
                    "/categoria/:categoria_id",
                    get(empresas::get_empresas_by_categoria),
                )
                .route(
                    "/horarios/:empresa_id",
                    get(empresas::get_horarios_by_empresa),
                )
                .route("/:id/horarios", put(empresas::update_horarios_empresa))
                .route("/:id/turnos", get(empresas::get_turnos_empresa))
                .route(
                    "/:id/atenciones",
                    get(atenciones::listar).post(atenciones::crear),
                )
                .route("/:id/atenciones/:atencion_id", delete(atenciones::eliminar))
                .route("/:id/verificado", put(moderacion::set_verificado))
                .route("/:id/verificacion", put(moderacion::set_verificacion))
                .route("/:id/reportes", post(moderacion::crear_reporte))
                .route(
                    "/:id/calificaciones",
                    get(empresas::get_calificaciones).post(empresas::calificar_empresa),
                )
                .route(
                    "/:id/telefonos",
                    get(empresas::get_telefonos_by_empresa).put(empresas::update_telefonos_empresa),
                )
                .route(
                    "/:id",
                    get(empresas::get_empresa_by_id)
                        .put(empresas::update_empresa)
                        .delete(empresas::delete_empresa),
                ),
        )
        .nest(
            "/publicidades",
            Router::new()
                .route("/", get(publicidades::listar).post(publicidades::crear))
                .route("/:id", delete(publicidades::eliminar)),
        )
        .nest(
            "/solicitudes-publicidad",
            Router::new()
                .route("/", post(solicitudes::crear).get(solicitudes::listar))
                .route("/:id", put(solicitudes::actualizar)),
        )
        .nest(
            "/reportes",
            Router::new()
                .route("/", get(moderacion::listar_reportes))
                .route("/:id/resolver", put(moderacion::resolver_reporte)),
        )
        .nest(
            "/push",
            Router::new()
                .route("/clave-publica", get(push::clave_publica))
                .route(
                    "/suscripciones",
                    post(push::suscribir).delete(push::desuscribir),
                ),
        )
        .nest(
            "/favoritos",
            Router::new().route("/", get(favoritos::listar)).route(
                "/:empresa_id",
                put(favoritos::agregar).delete(favoritos::quitar),
            ),
        )
        .nest(
            "/turnos",
            Router::new()
                .route("/reservar", post(turnos::reservar_turno))
                .route("/mis-turnos", get(turnos::get_mis_turnos))
                .route("/ocupados", get(turnos::get_turnos_ocupados))
                .route("/cancelar/:id", delete(turnos::cancelar_turno)),
        );

    Router::new()
        .route("/", get(|| async { "✅ Backend corriendo correctamente" }))
        .route("/health", get(health::health))
        .route("/health/live", get(health::live))
        .nest("/api", api)
        .with_state(state)
}
