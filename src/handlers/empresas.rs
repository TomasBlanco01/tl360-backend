use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, FromRow, PgPool};
use std::collections::{HashMap, HashSet};

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;
use crate::validation::{telefono_valido, texto_hasta, texto_hasta_opt, url_http, url_http_opt};

/// Ficha del comercio. Los teléfonos, las redes sociales y las fotos viven en sus propias tablas
/// (telefono_empresa, red_social, foto_empresa) y viajan como listas, igual que en el modelo.
#[derive(FromRow, Serialize)]
pub struct Empresa {
    id: i32,
    nombre: String,
    slug: Option<String>,
    direccion: String,
    slogan: Option<String>,
    duracion_turno_min: Option<i32>,
    rubro_id: i32,
    usuario_id: Option<i32>,
    /// telefono_empresa: el principal primero.
    telefonos: SqlJson<Vec<Telefono>>,
    /// red_social: una por tipo.
    redes_sociales: SqlJson<Vec<RedSocial>>,
    /// foto_empresa: URLs en el orden de la galería.
    fotos: Vec<String>,
    turnos_habilitados: bool,
    verificado: bool,
    verificado_estado: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cuit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url_cuit: Option<String>,
    /// RF-15: por qué se rechazó la verificación; solo existe mientras está rechazada.
    #[serde(skip_serializing_if = "Option::is_none")]
    motivo_rechazo: Option<String>,
    lat: Option<f64>,
    lng: Option<f64>,
    modo_contacto: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    descripcion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rubro: Option<String>,
    /// RF-18: etiquetas descriptivas del comercio, ordenadas alfabéticamente.
    etiquetas: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calificacion_promedio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calificacion_cantidad: Option<i64>,
    /// RF-07: distancia al punto buscado, solo cuando se envía `?lat=&lng=`.
    #[serde(skip_serializing_if = "Option::is_none")]
    distancia_km: Option<f64>,
    /// RF-07: si el comercio está en su franja horaria ahora, para ordenar abiertos primero.
    #[serde(skip_serializing_if = "Option::is_none")]
    abierto_ahora: Option<bool>,
}

#[derive(FromRow, Serialize, Deserialize)]
pub struct Telefono {
    id: i32,
    numero: String,
    descripcion: Option<String>,
    /// Canal de ese número; NULL hereda el `modo_contacto` del comercio.
    canal: Option<String>,
    es_whatsapp: bool,
    /// El número que se muestra primero en la ficha (uno por comercio).
    principal: bool,
}

#[derive(Serialize, Deserialize)]
pub struct RedSocial {
    tipo: String,
    url: String,
}

const SELECT_TELEFONOS: &str = "SELECT id, numero, descripcion, canal, es_whatsapp, principal \
     FROM telefono_empresa WHERE empresa_id = $1 ORDER BY principal DESC, id";

/// SELECT común de lectura. En el listado se omite la descripción (texto largo × miles de comercios)
/// y se devuelve completa solo en la ficha individual.
/// Distancia haversine en km para el punto `$8`/`$9`; NULL si el comercio no tiene coordenadas.
const DISTANCIA_KM: &str = "CASE WHEN e.lat IS NULL OR e.lng IS NULL THEN NULL ELSE \
    6371.0 * 2.0 * asin(LEAST(1.0, sqrt( \
      power(sin(radians(e.lat::float8 - $8::float8) / 2.0), 2) + \
      cos(radians($8::float8)) * cos(radians(e.lat::float8)) * \
      power(sin(radians(e.lng::float8 - $9::float8) / 2.0), 2) \
    ))) END";

/// RF-07: usa el día/hora local (`$10`/`$11`) para saber si el comercio está abierto.
const ABIERTO_AHORA: &str = "EXISTS (SELECT 1 FROM horario_empresa h \
    WHERE h.empresa_id = e.id AND h.dia_semana = $10::text \
      AND $11::time >= h.hora_inicio AND $11::time < h.hora_fin)";

fn read_select(con_descripcion: bool, con_distancia: bool) -> String {
    let desc = if con_descripcion {
        "e.descripcion::text"
    } else {
        "NULL::text"
    };
    let (distancia, abierto) = if con_distancia {
        (
            format!("{DISTANCIA_KM} AS distancia_km"),
            format!("{ABIERTO_AHORA} AS abierto_ahora"),
        )
    } else {
        (
            "NULL::float8 AS distancia_km".to_string(),
            "NULL::bool AS abierto_ahora".to_string(),
        )
    };
    format!(
        "SELECT e.id, e.nombre, e.slug, e.direccion, \
         e.slogan, e.duracion_turno_min::int4 AS duracion_turno_min, e.rubro_id, e.usuario_id, \
         COALESCE((SELECT jsonb_agg(jsonb_build_object('id', t.id, 'numero', t.numero, \
                     'descripcion', t.descripcion, 'canal', t.canal, 'es_whatsapp', t.es_whatsapp, \
                     'principal', t.principal) ORDER BY t.principal DESC, t.id) \
                   FROM telefono_empresa t WHERE t.empresa_id = e.id), '[]'::jsonb) AS telefonos, \
         COALESCE((SELECT jsonb_agg(jsonb_build_object('tipo', rs.tipo, 'url', rs.url) ORDER BY rs.tipo) \
                   FROM red_social rs WHERE rs.empresa_id = e.id), '[]'::jsonb) AS redes_sociales, \
         COALESCE((SELECT array_agg(f.url::text ORDER BY f.orden, f.id) FROM foto_empresa f \
                   WHERE f.empresa_id = e.id), ARRAY[]::text[]) AS fotos, \
         e.turnos_habilitados, e.verificado, e.verificado_estado, \
         e.cuit, e.url_cuit, e.motivo_rechazo, e.lat::float8 AS lat, e.lng::float8 AS lng, \
         e.modo_contacto, {desc} AS descripcion, r.nombre AS rubro, \
         COALESCE((SELECT array_agg(et.nombre::text ORDER BY et.nombre) \
                   FROM empresa_etiqueta ee JOIN etiqueta et ON et.id = ee.etiqueta_id \
                   WHERE ee.empresa_id = e.id), ARRAY[]::text[]) AS etiquetas, \
         (SELECT ROUND(AVG(k.puntaje)::numeric, 1)::float8 FROM calificacion k WHERE k.empresa_id = e.id) AS calificacion_promedio, \
         (SELECT COUNT(*) FROM calificacion k WHERE k.empresa_id = e.id) AS calificacion_cantidad, \
         {distancia}, {abierto} \
         FROM empresa e LEFT JOIN rubro r ON r.id = e.rubro_id"
    )
}

impl Empresa {
    /// Datos de la verificación comercial (CUIT, constancia fiscal, motivo de rechazo y si la
    /// solicitud está pendiente o fue rechazada): solo los ven el dueño del comercio y el
    /// superadmin. El resto ve únicamente si tiene el sello de verificado.
    fn para(mut self, user: Option<&CurrentUser>) -> Self {
        let autorizado =
            user.is_some_and(|u| u.rol == "superadmin" || Some(u.id) == self.usuario_id);
        if !autorizado {
            self.cuit = None;
            self.url_cuit = None;
            self.motivo_rechazo = None;
            self.verificado_estado = if self.verificado {
                "verificado".into()
            } else {
                "sin_solicitar".into()
            };
        }
        self
    }
}

fn para_todos(rows: Vec<Empresa>, user: Option<&CurrentUser>) -> Vec<Empresa> {
    rows.into_iter().map(|e| e.para(user)).collect()
}

async fn fetch_empresa(db: &PgPool, id: i32) -> AppResult<Empresa> {
    sqlx::query_as::<_, Empresa>(&format!("{} WHERE e.id = $1", read_select(true, false)))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound("Empresa no encontrada".into()))
}

#[derive(Deserialize)]
pub struct ListQuery {
    q: Option<String>,
    rubro_id: Option<i32>,
    usuario_id: Option<i32>,
    verificado_estado: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    /// RF-07: punto de referencia para ordenar por cercanía. Se envían juntos.
    lat: Option<f64>,
    lng: Option<f64>,
    /// RF-18: etiquetas separadas por coma; se exigen todas (AND).
    etiquetas: Option<String>,
    /// Ids separados por coma (hasta 200), en ese orden: el mapa pide las fichas de los comercios
    /// que quedan a la vista.
    ids: Option<String>,
}

const MAX_IDS: usize = 200;

fn ids_filtro(raw: &Option<String>) -> AppResult<Option<Vec<i32>>> {
    let Some(raw) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let ids = raw
        .split(',')
        .map(|s| s.trim().parse::<i32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::BadRequest("ids debe ser una lista de números separados por coma".into()))?;
    if ids.len() > MAX_IDS {
        return Err(AppError::BadRequest(format!("Se piden hasta {MAX_IDS} comercios por vez")));
    }
    Ok(Some(ids))
}

/// Catálogo canónico de RF-18 y sus alias habituales (claves sin acentos ni signos).
const CATALOGO_ETIQUETAS: [(&str, &[&str]); 5] = [
    (
        "Rampa de acceso",
        &[
            "rampa",
            "rampadeacceso",
            "accesoparasillasderuedas",
            "sillasderuedas",
        ],
    ),
    (
        "Baño adaptado",
        &["bano", "banio", "banoadaptado", "banioadaptado"],
    ),
    (
        "Apto celíacos",
        &["aptoceliaco", "aptoceliacos", "celiaco", "celiacos"],
    ),
    (
        "Acepta mascotas",
        &[
            "mascota",
            "mascotas",
            "aceptamascota",
            "aceptamascotas",
            "petfriendly",
        ],
    ),
    (
        "Wi-Fi gratis",
        &["wifi", "wifigratis", "internet", "internetgratis"],
    ),
];

const MAX_ETIQUETAS: usize = 12;
const MAX_LARGO_ETIQUETA: usize = 40;

/// Clave de comparación: minúsculas, sin acentos ni signos.
fn clave_etiqueta(valor: &str) -> String {
    valor
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            _ => c,
        })
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Resuelve alias conocidos al nombre canónico; deja intactas las etiquetas libres.
fn etiqueta_canonica(valor: &str) -> String {
    let clave = clave_etiqueta(valor);
    CATALOGO_ETIQUETAS
        .iter()
        .find(|(_, alias)| alias.contains(&clave.as_str()))
        .map_or_else(
            || valor.trim().to_string(),
            |(nombre, _)| (*nombre).to_string(),
        )
}

fn etiquetas_filtro(raw: &Option<String>) -> Option<Vec<String>> {
    let mut vistas = HashSet::new();
    let mut salida = Vec::new();
    for pedida in raw.as_deref().unwrap_or_default().split(',') {
        if pedida.trim().is_empty() {
            continue;
        }
        let nombre = etiqueta_canonica(pedida);
        if !nombre.is_empty() && vistas.insert(clave_etiqueta(&nombre)) {
            salida.push(nombre);
        }
    }
    (!salida.is_empty()).then_some(salida)
}

/// `None` conserva las etiquetas actuales; `Some([])` las borra.
fn normalizar_etiquetas(valor: &Option<Vec<String>>) -> AppResult<Option<Vec<String>>> {
    let Some(lista) = valor else {
        return Ok(None);
    };
    if lista.len() > MAX_ETIQUETAS {
        return Err(AppError::BadRequest(format!(
            "Se permiten hasta {MAX_ETIQUETAS} etiquetas por comercio"
        )));
    }
    let mut vistas = HashSet::new();
    let mut salida = Vec::new();
    for etiqueta in lista {
        let nombre = etiqueta_canonica(etiqueta);
        if nombre.is_empty() {
            continue;
        }
        if nombre.chars().count() > MAX_LARGO_ETIQUETA {
            return Err(AppError::BadRequest(format!(
                "Cada etiqueta puede tener hasta {MAX_LARGO_ETIQUETA} caracteres"
            )));
        }
        if vistas.insert(clave_etiqueta(&nombre)) {
            salida.push(nombre);
        }
    }
    Ok(Some(salida))
}

/// Sin `limit` se devuelve el catálogo completo (hasta este tope) por compatibilidad con el front actual.
const LIMITE_MAXIMO: i64 = 5000;

const ORDEN_LISTADO: &str = "ORDER BY e.verificado DESC, e.turnos_habilitados DESC, e.id";

/// RF-07: primero los geolocalizados, luego abiertos, y dentro de cada grupo los más cercanos.
const ORDEN_CERCANIA: &str = "ORDER BY (e.lat IS NULL OR e.lng IS NULL), abierto_ahora DESC, \
    distancia_km ASC, e.verificado DESC, e.turnos_habilitados DESC, e.id";

fn coordenadas(f: &ListQuery) -> AppResult<Option<(f64, f64)>> {
    match (f.lat, f.lng) {
        (None, None) => Ok(None),
        (Some(lat), Some(lng))
            if lat.is_finite()
                && lng.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lng) =>
        {
            Ok(Some((lat, lng)))
        }
        (Some(_), Some(_)) => Err(AppError::BadRequest(
            "Las coordenadas están fuera de rango".into(),
        )),
        _ => Err(AppError::BadRequest(
            "Enviá las dos coordenadas (lat y lng) juntas para ordenar por cercanía".into(),
        )),
    }
}

/// Minúsculas, sin tildes y sin puntuación: la misma forma que usa el diccionario.
fn normalizar_busqueda(texto: &str) -> String {
    let mut salida = String::with_capacity(texto.len());
    for c in texto.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            otro => otro,
        };
        salida.push(if c.is_alphanumeric() || c == ' ' {
            c
        } else {
            ' '
        });
    }
    salida.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// RF-07: reglas locales de lenguaje natural; no requiere un modelo de IA. Los sinónimos se buscan
/// como palabra completa (con su plural): "bar" encuentra "Bares" pero no "Barberías".
fn sinonimos_busqueda(palabra: &str) -> &'static [&'static str] {
    match palabra {
        "comida" | "comidas" | "comer" => &[
            "comida",
            "gastronomia",
            "restaurante",
            "bar",
            "rotiseria",
            "pizzeria",
            "parrilla",
            "cafeteria",
            "catering",
            "pastas",
        ],
        "gastronomia" => &["gastronomia", "restaurante", "comida"],
        "restaurante" | "restaurant" => &["gastronomia", "restaurante", "comida", "bar", "parrilla"],
        "peluqueria" => &[
            "peluqueria",
            "barberia",
            "corte",
            "pelo",
            "peinado",
            "color",
        ],
        "barberia" => &["barberia", "peluqueria", "corte", "barba", "fade"],
        "corte" => &["peluqueria", "barberia", "corte", "pelo"],
        "pelo" => &["peluqueria", "barberia", "pelo", "color"],
        "barba" => &["barberia", "barba", "fade"],
        "fade" => &["barberia", "fade", "corte"],
        "gym" | "gimnasio" => &["gimnasio", "fitness", "entrenamiento", "gym"],
        "pesas" => &["gimnasio", "fitness", "entrenamiento", "musculacion"],
        "fitness" => &["gimnasio", "fitness", "entrenamiento"],
        "crossfit" => &["gimnasio", "crossfit", "funcional"],
        "funcional" => &["gimnasio", "funcional", "entrenamiento"],
        "unas" | "manicuria" => &["manicuria", "pedicura", "unas", "nails"],
        "masaje" | "masajes" => &["masaje", "masajes", "relax", "spa"],
        "dentista" => &["odontologia", "dental"],
        "medico" => &["salud", "clinica", "consultorio"],
        "perro" | "perros" | "gato" | "gatos" => &["veterinaria"],
        "carne" | "carnes" | "asado" => &["carniceria", "frigorifico"],
        "pizza" | "pizzas" => &["pizzeria", "gastronomia"],
        "kiosco" | "almacen" => &["almacen", "kiosco", "drugstore"],
        "ropa" | "indumentaria" => &["indumentaria", "ropa", "calzado", "tienda"],
        "farmacia" => &["farmacia", "medicamento"],
        "yoga" | "pilates" => &["yoga", "pilates", "meditacion"],
        "tatuaje" | "tattoo" => &["tatuaje", "tattoo", "tinta"],
        _ => &[],
    }
}

/// Palabras que no aportan a la búsqueda ("corte de pelo" no debe buscar "de" en todos lados).
const PALABRAS_VACIAS: [&str; 16] = [
    "de", "del", "la", "las", "el", "los", "un", "una", "con", "para", "por", "en", "y", "o", "a",
    "al",
];

/// Términos de búsqueda, con un prefijo que dice cómo se buscan:
/// - sin prefijo: lo escrito, como comienzo de palabra ("pelu" encuentra "Peluquería", pero "uñas"
///   no encuentra "vacunas"), en nombre, rubro, etiquetas y descripción;
/// - `!`: lo escrito cuando es una palabra conocida del diccionario, como palabra completa (con su
///   plural), así "comer" no trae "Comercio", "perro" no trae "Perroud" ni "carne" trae "carnet";
/// - `=`: un sinónimo, como palabra completa y solo en el nombre y el rubro, para que "comida" no
///   traiga barberías (por "bar") y "perro" no traiga todo comercio con la etiqueta "Acepta mascotas".
fn expandir_busqueda(q: &str) -> Option<Vec<String>> {
    let base = normalizar_busqueda(q);
    if base.is_empty() {
        return None;
    }
    let mut terminos: Vec<String> = Vec::new();
    let mut agregar = |valor: String| {
        let palabra = valor.trim_start_matches(['=', '!']);
        if palabra.is_empty() {
            return;
        }
        // Lo escrito (sin prefijo o con `!`) ya cubre al sinónimo igual.
        if terminos.iter().any(|t| t.trim_start_matches(['=', '!']) == palabra) {
            return;
        }
        terminos.push(valor);
    };
    let escrito = |palabra: &str| {
        if sinonimos_busqueda(palabra).is_empty() {
            palabra.to_string()
        } else {
            format!("!{palabra}")
        }
    };
    agregar(escrito(&base));
    for palabra in base.split(' ') {
        if palabra.chars().count() < 3 || PALABRAS_VACIAS.contains(&palabra) {
            continue;
        }
        agregar(escrito(palabra));
        for sinonimo in sinonimos_busqueda(palabra) {
            agregar(format!("={}", normalizar_busqueda(sinonimo)));
        }
    }
    for sinonimo in sinonimos_busqueda(&base) {
        agregar(format!("={}", normalizar_busqueda(sinonimo)));
    }
    Some(terminos)
}

/// Palabra completa (con su plural) para un término sin su prefijo.
fn palabra_completa(columna: &str) -> String {
    format!("{columna} ~ ('\\m' || ltrim(buscado.termino, '=!') || '(s|es)?\\M')")
}

/// Condición SQL para un término de `expandir_busqueda` sobre una columna ya normalizada.
fn coincide_termino(columna: &str) -> String {
    format!(
        "(CASE WHEN left(buscado.termino, 1) IN ('=', '!') THEN {} \
            ELSE {columna} ~ ('\\m' || buscado.termino) END)",
        palabra_completa(columna)
    )
}

/// Filtros compartidos por el listado y el mapa (búsqueda RF-07, rubro y etiquetas RF-18), con los
/// números de parámetro de cada consulta. Requiere `empresa e LEFT JOIN rubro r`.
fn filtro_directorio(p_busqueda: u8, p_rubro: u8, p_etiquetas: u8) -> String {
    let norm = |col: &str| format!("translate(lower({col}), 'áéíóúüñ', 'aeiouun')");
    format!(
        "(${p_busqueda}::text[] IS NULL OR EXISTS ( \
             SELECT 1 FROM unnest(${p_busqueda}::text[]) AS buscado(termino) \
             WHERE {} OR {} OR {} \
                OR EXISTS ( \
                    SELECT 1 FROM empresa_etiqueta ee JOIN etiqueta et ON et.id = ee.etiqueta_id \
                    WHERE ee.empresa_id = e.id AND left(buscado.termino, 1) <> '=' AND {} \
                ) \
         )) \
         AND (${p_rubro}::int4 IS NULL OR e.rubro_id = ${p_rubro}) \
         AND (${p_etiquetas}::text[] IS NULL OR NOT EXISTS ( \
             SELECT 1 FROM unnest(${p_etiquetas}::text[]) AS pedida(nombre) \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM empresa_etiqueta ee JOIN etiqueta et ON et.id = ee.etiqueta_id \
                 WHERE ee.empresa_id = e.id AND lower(et.nombre) = lower(pedida.nombre) \
             ) \
         ))",
        coincide_termino(&norm("e.nombre")),
        coincide_termino(&norm("COALESCE(r.nombre, '')")),
        // En la descripción solo se busca lo que escribió el usuario: un sinónimo como "parrilla"
        // aparece en herrerías que las fabrican y no debe traerlas al buscar "comida".
        format!(
            "(left(buscado.termino, 1) <> '=' AND {})",
            coincide_termino(&norm("COALESCE(e.descripcion, '')"))
        ),
        // Los signos se comparan como espacio ("wi-fi") y también pegados ("wifi"), igual que se
        // normaliza lo que escribe el usuario: las dos encuentran "Wi-Fi gratis".
        format!(
            "({} OR {})",
            coincide_termino(&format!(
                "regexp_replace({}, '[^a-z0-9 ]', ' ', 'g')",
                norm("et.nombre")
            )),
            coincide_termino(&format!(
                "regexp_replace({}, '[^a-z0-9 ]', '', 'g')",
                norm("et.nombre")
            )),
        ),
    )
}

/// RF-07: con búsqueda, el orden es
/// 0. lo escrito como palabra completa en el nombre o el rubro ("bar" → Bares, "comida" → Casas de
///    Comidas);
/// 1. lo escrito como comienzo de palabra en el nombre o el rubro ("bar" → Barberías);
/// 2. un sinónimo en el nombre o el rubro ("comida" → Pizzerías);
/// 3. lo que solo lo menciona en la descripción o en una etiqueta.
fn orden_relevancia(p_busqueda: u8) -> String {
    let norm = |col: &str| format!("translate(lower({col}), 'áéíóúüñ', 'aeiouun')");
    let nombre = norm("e.nombre");
    let rubro = norm("COALESCE(r.nombre, '')");
    let existe = |condicion: String| {
        format!(
            "EXISTS (SELECT 1 FROM unnest(${p_busqueda}::text[]) AS buscado(termino) \
             WHERE {condicion})"
        )
    };
    let escrito = "left(buscado.termino, 1) <> '='";
    format!(
        "(CASE WHEN ${p_busqueda}::text[] IS NULL THEN 0 WHEN {} THEN 0 WHEN {} THEN 1 \
          WHEN {} THEN 2 ELSE 3 END)",
        existe(format!(
            "{escrito} AND ({} OR {})",
            palabra_completa(&nombre),
            palabra_completa(&rubro)
        )),
        existe(format!(
            "{escrito} AND ({} OR {})",
            coincide_termino(&nombre),
            coincide_termino(&rubro)
        )),
        existe(format!(
            "{} OR {}",
            coincide_termino(&nombre),
            coincide_termino(&rubro)
        )),
    )
}

pub async fn get_empresas(
    user: Option<CurrentUser>,
    State(state): State<AppState>,
    Query(f): Query<ListQuery>,
) -> AppResult<Json<Vec<Empresa>>> {
    // La bandeja de moderación (pendientes, rechazados) es solo del superadmin.
    if f.verificado_estado.as_deref().is_some_and(|s| !s.trim().is_empty())
        && !user.as_ref().is_some_and(|u| u.rol == "superadmin")
    {
        return Err(AppError::sin_permisos());
    }
    let geo = coordenadas(&f)?;
    let etiquetas = etiquetas_filtro(&f.etiquetas);
    let busqueda = f.q.as_deref().and_then(expandir_busqueda);
    let ids = ids_filtro(&f.ids)?;
    let orden = if ids.is_some() {
        "ORDER BY array_position($12::int4[], e.id)".to_string()
    } else if geo.is_some() {
        ORDEN_CERCANIA.to_string()
    } else {
        ORDEN_LISTADO.replacen("ORDER BY ", &format!("ORDER BY {}, ", orden_relevancia(1)), 1)
    };
    let sql = format!(
        "{} WHERE {} \
         AND ($5::int4 IS NULL OR e.usuario_id = $5) \
         AND ($6::text IS NULL OR e.verificado_estado = $6) \
         AND ($12::int4[] IS NULL OR e.id = ANY($12::int4[])) \
         {orden} LIMIT $3::int8 OFFSET $4::int8",
        read_select(false, geo.is_some()),
        filtro_directorio(1, 2, 7),
    );
    let ahora = crate::agenda::ahora_local();
    // $8-$11 solo se usan al ordenar por cercanía; se envían siempre (tipados) para que $12 exista.
    let rows = sqlx::query_as::<_, Empresa>(&sql)
        .bind(busqueda)
        .bind(f.rubro_id)
        .bind(f.limit.map_or(LIMITE_MAXIMO, |l| l.clamp(1, 1000)))
        .bind(f.offset.unwrap_or(0).max(0))
        .bind(f.usuario_id)
        .bind(
            f.verificado_estado
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(etiquetas)
        .bind(geo.map(|g| g.0))
        .bind(geo.map(|g| g.1))
        .bind(crate::agenda::nombre_dia(ahora.date()))
        .bind(ahora.time())
        .bind(ids)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(para_todos(rows, user.as_ref())))
}

pub async fn get_empresas_by_rubro(
    user: Option<CurrentUser>,
    State(state): State<AppState>,
    Path(rubro_id): Path<i32>,
) -> AppResult<Json<Vec<Empresa>>> {
    let rows = sqlx::query_as::<_, Empresa>(&format!(
        "{} WHERE e.rubro_id = $1 {ORDEN_LISTADO}",
        read_select(false, false)
    ))
    .bind(rubro_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(para_todos(rows, user.as_ref())))
}

/// La ficha pública se comparte por slug (`/comercios/peluqueria-central`), pero las
/// operaciones internas siguen usando el ID numérico. Aceptamos ambos en las lecturas.
async fn resolver_empresa_id(db: &PgPool, id_o_slug: &str) -> AppResult<i32> {
    let criterio = id_o_slug.trim();
    if criterio.is_empty() {
        return Err(AppError::NotFound("Empresa no encontrada".into()));
    }
    let id = sqlx::query_scalar::<_, i32>(
        "SELECT id FROM empresa WHERE id::text = $1 OR slug = $1 \
         ORDER BY (id::text = $1) DESC LIMIT 1",
    )
    .bind(criterio)
    .fetch_optional(db)
    .await?;
    id.ok_or_else(|| AppError::NotFound("Empresa no encontrada".into()))
}

pub async fn get_empresa_by_id(
    user: Option<CurrentUser>,
    State(state): State<AppState>,
    Path(id_o_slug): Path<String>,
) -> AppResult<Json<Empresa>> {
    let id = resolver_empresa_id(&state.db, &id_o_slug).await?;
    Ok(Json(
        fetch_empresa(&state.db, id).await?.para(user.as_ref()),
    ))
}

pub async fn get_telefonos_by_empresa(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Vec<Telefono>>> {
    let rows = sqlx::query_as::<_, Telefono>(SELECT_TELEFONOS)
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

const DIAS: [&str; 7] = [
    "Lunes",
    "Martes",
    "Miércoles",
    "Jueves",
    "Viernes",
    "Sábado",
    "Domingo",
];

#[derive(FromRow)]
struct HorarioRow {
    dia_semana: String,
    hora_inicio: NaiveTime,
    hora_fin: NaiveTime,
}

async fn horarios_de(db: &PgPool, empresa_id: i32) -> AppResult<HashMap<String, Vec<String>>> {
    let rows = sqlx::query_as::<_, HorarioRow>(
        "SELECT dia_semana, hora_inicio, hora_fin FROM horario_empresa WHERE empresa_id = $1 ORDER BY hora_inicio",
    )
    .bind(empresa_id)
    .fetch_all(db)
    .await?;

    let mut horarios: HashMap<String, Vec<String>> = HashMap::new();
    for h in rows {
        // Formato "HH:MM", igual al que usa el prototipo mock (ver mock/data.ts).
        let rango = format!(
            "{}-{}",
            h.hora_inicio.format("%H:%M"),
            h.hora_fin.format("%H:%M")
        );
        let dia = horarios.entry(h.dia_semana.trim().to_string()).or_default();
        if !dia.contains(&rango) {
            dia.push(rango);
        }
    }
    Ok(horarios)
}

pub async fn get_horarios_by_empresa(
    State(state): State<AppState>,
    Path(empresa_id_o_slug): Path<String>,
) -> AppResult<Json<HashMap<String, Vec<String>>>> {
    let empresa_id = resolver_empresa_id(&state.db, &empresa_id_o_slug).await?;
    Ok(Json(horarios_de(&state.db, empresa_id).await?))
}

#[derive(Deserialize)]
pub struct HorariosRequest {
    horarios: HashMap<String, Vec<String>>,
}

fn parse_rango(rango: &str) -> AppResult<(NaiveTime, NaiveTime)> {
    let malo = || {
        AppError::BadRequest(format!(
            "Franja horaria inválida: '{rango}' (se espera HH:MM-HH:MM)"
        ))
    };
    let (a, b) = rango.split_once('-').ok_or_else(malo)?;
    let ini = NaiveTime::parse_from_str(a.trim(), "%H:%M").map_err(|_| malo())?;
    let fin = NaiveTime::parse_from_str(b.trim(), "%H:%M").map_err(|_| malo())?;
    if fin <= ini {
        return Err(AppError::BadRequest(format!(
            "La franja '{rango}' termina antes de empezar"
        )));
    }
    Ok((ini, fin))
}

/// Dos franjas del mismo día no pueden pisarse: la grilla de turnos quedaría ambigua. Que una
/// termine justo cuando empieza la otra (turno partido sin corte) sí se permite.
fn validar_sin_superposicion(filas: &[(&str, NaiveTime, NaiveTime)]) -> AppResult<()> {
    let mut ordenadas = filas.to_vec();
    ordenadas.sort();
    for par in ordenadas.windows(2) {
        let ((dia_a, ini_a, fin_a), (dia_b, ini_b, fin_b)) = (par[0], par[1]);
        if dia_a == dia_b && ini_b < fin_a {
            return Err(AppError::BadRequest(format!(
                "Las franjas {}-{} y {}-{} del {dia_a} se superponen",
                ini_a.format("%H:%M"),
                fin_a.format("%H:%M"),
                ini_b.format("%H:%M"),
                fin_b.format("%H:%M")
            )));
        }
    }
    Ok(())
}

pub async fn update_horarios_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<HorariosRequest>,
) -> AppResult<Json<HashMap<String, Vec<String>>>> {
    // Se valida todo antes de tocar la base, para no dejar horarios a medio reemplazar.
    let mut filas: Vec<(&str, NaiveTime, NaiveTime)> = Vec::new();
    for (dia, rangos) in &body.horarios {
        let dia = DIAS
            .iter()
            .find(|d| **d == dia.as_str())
            .ok_or_else(|| AppError::BadRequest(format!("Día inválido: '{dia}'")))?;
        for r in rangos {
            let (ini, fin) = parse_rango(r)?;
            filas.push((dia, ini, fin));
        }
    }
    validar_sin_superposicion(&filas)?;
    verificar_dueno(&state, &user, id).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM horario_empresa WHERE empresa_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for (dia, ini, fin) in filas {
        sqlx::query("INSERT INTO horario_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(dia)
            .bind(ini)
            .bind(fin)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(horarios_de(&state.db, id).await?))
}

#[derive(Deserialize)]
pub struct EmpresaRequest {
    nombre: String,
    direccion: String,
    slogan: Option<String>,
    duracion_turno_min: Option<i32>,
    rubro_id: i32,
    usuario_id: Option<i32>,
    /// Teléfonos (tabla telefono_empresa): ausente conserva los actuales; la lista los reemplaza.
    telefonos: Option<Vec<TelefonoInput>>,
    /// Redes sociales (tabla red_social): ausente conserva las actuales; la lista las reemplaza.
    redes_sociales: Option<Vec<RedSocial>>,
    /// Galería (tabla foto_empresa): ausente conserva las actuales.
    fotos: Option<Vec<String>>,
    /// RF-18: ausente conserva las actuales; `[]` las borra.
    etiquetas: Option<Vec<String>>,
    turnos_habilitados: Option<bool>,
    descripcion: Option<String>,
    cuit: Option<String>,
    url_cuit: Option<String>,
    verificado_estado: Option<String>,
    /// RF-04: punto elegido en el mapa; se envían juntos o ambos quedan vacíos.
    lat: Option<f64>,
    lng: Option<f64>,
    /// RF-02: canal de contacto preferido del comercio.
    modo_contacto: Option<String>,
}

const ESTADOS_VERIFICACION: [&str; 4] = ["sin_solicitar", "pendiente", "verificado", "rechazado"];
const MODOS_CONTACTO: [&str; 3] = ["llamada", "mensaje", "ambos"];
const MAX_FOTOS: usize = 20;

fn validar_coordenadas(lat: Option<f64>, lng: Option<f64>) -> AppResult<()> {
    match (lat, lng) {
        (None, None) => Ok(()),
        (Some(lat), Some(lng))
            if lat.is_finite()
                && lng.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lng) =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err(AppError::BadRequest(
            "Las coordenadas están fuera de rango".into(),
        )),
        _ => Err(AppError::BadRequest(
            "Latitud y longitud deben enviarse juntas".into(),
        )),
    }
}

fn validar_modo_contacto(valor: &Option<String>) -> AppResult<Option<&str>> {
    match valor.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) if MODOS_CONTACTO.contains(&v) => Ok(Some(v)),
        Some(_) => Err(AppError::BadRequest(
            "El modo de contacto debe ser 'llamada', 'mensaje' o 'ambos'".into(),
        )),
    }
}

/// CUIT: se acepta con guiones o espacios y se guarda solo con sus 11 dígitos.
pub(crate) fn normalizar_cuit(valor: &Option<String>) -> AppResult<Option<String>> {
    match valor.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => {
            let digitos: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
            if digitos.len() != 11 {
                return Err(AppError::BadRequest("El CUIT debe tener 11 dígitos".into()));
            }
            Ok(Some(digitos))
        }
    }
}

fn validar_estado_verificacion(valor: &Option<String>) -> AppResult<Option<&str>> {
    match valor.as_deref() {
        None => Ok(None),
        // RF-15: rechazar exige un motivo, que solo se carga desde la bandeja de verificación.
        Some("rechazado") => Err(AppError::BadRequest(
            "Para rechazar una verificación usá la bandeja de verificación e indicá el motivo".into(),
        )),
        Some(v) if ESTADOS_VERIFICACION.contains(&v) => Ok(Some(v)),
        Some(_) => Err(AppError::BadRequest(
            "Estado de verificación inválido".into(),
        )),
    }
}

/// Texto opcional sin espacios sobrantes; vacío equivale a no informado.
fn limpio(valor: &Option<String>) -> Option<&str> {
    valor.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn validar_ficha(b: &EmpresaRequest) -> AppResult<()> {
    if b.nombre.trim().is_empty() || b.direccion.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Nombre y dirección son obligatorios".into(),
        ));
    }
    // Los mismos máximos que las columnas VARCHAR(N) del modelo.
    texto_hasta("El nombre", &b.nombre, 100)?;
    texto_hasta("La dirección", &b.direccion, 150)?;
    texto_hasta_opt("El slogan", &b.slogan, 150)?;
    texto_hasta_opt("La descripción", &b.descripcion, 2000)?;
    if let Some(telefonos) = &b.telefonos {
        validar_telefonos(telefonos)?;
    }
    if let Some(redes) = &b.redes_sociales {
        validar_redes(redes)?;
    }
    url_http_opt("url_cuit", &b.url_cuit)?;
    normalizar_cuit(&b.cuit)?;
    validar_estado_verificacion(&b.verificado_estado)?;
    validar_coordenadas(b.lat, b.lng)?;
    validar_modo_contacto(&b.modo_contacto)?;
    if let Some(d) = b.duracion_turno_min {
        if !(1..=480).contains(&d) {
            return Err(AppError::BadRequest(
                "La duración del turno debe estar entre 1 y 480 minutos".into(),
            ));
        }
    }
    let fotos = b.fotos.as_deref().unwrap_or_default();
    if fotos.len() > MAX_FOTOS {
        return Err(AppError::BadRequest(format!(
            "Se permiten hasta {MAX_FOTOS} fotos por comercio"
        )));
    }
    for f in fotos {
        url_http("foto", f)?;
    }
    Ok(())
}

/// RF-18: reemplaza las etiquetas dentro de la misma transacción que la ficha.
async fn reemplazar_etiquetas(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    empresa_id: i32,
    etiquetas: &[String],
) -> AppResult<()> {
    sqlx::query("DELETE FROM empresa_etiqueta WHERE empresa_id = $1")
        .bind(empresa_id)
        .execute(&mut **tx)
        .await?;
    for nombre in etiquetas {
        let etiqueta_id = sqlx::query_scalar::<_, i32>(
            "INSERT INTO etiqueta (nombre) VALUES ($1) \
             ON CONFLICT (nombre) DO UPDATE SET nombre = EXCLUDED.nombre RETURNING id",
        )
        .bind(nombre)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO empresa_etiqueta (empresa_id, etiqueta_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(empresa_id)
        .bind(etiqueta_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

const MAX_TELEFONOS: usize = 10;
/// Los mismos tipos que admite el CHECK de red_social.
const TIPOS_RED_SOCIAL: [&str; 6] = ["instagram", "facebook", "tiktok", "x", "youtube", "web"];

/// Lista de teléfonos de la ficha: números válidos, sin repetir y con un solo principal.
fn validar_telefonos(telefonos: &[TelefonoInput]) -> AppResult<()> {
    if telefonos.len() > MAX_TELEFONOS {
        return Err(AppError::BadRequest(format!(
            "Se permiten hasta {MAX_TELEFONOS} teléfonos por comercio"
        )));
    }
    let mut vistos = HashSet::new();
    for t in telefonos {
        telefono_valido(&t.numero)?;
        texto_hasta_opt("La descripción del teléfono", &t.descripcion, 50)?;
        validar_modo_contacto(&t.canal)?;
        if !vistos.insert(t.numero.trim().to_string()) {
            return Err(AppError::BadRequest(format!(
                "El teléfono {} está repetido",
                t.numero.trim()
            )));
        }
    }
    if telefonos.iter().filter(|t| t.principal == Some(true)).count() > 1 {
        return Err(AppError::BadRequest(
            "Solo un teléfono puede ser el principal".into(),
        ));
    }
    Ok(())
}

/// Redes sociales: tipos conocidos, una por tipo y URL http(s).
fn validar_redes(redes: &[RedSocial]) -> AppResult<()> {
    let mut vistos = HashSet::new();
    for r in redes {
        if !TIPOS_RED_SOCIAL.contains(&r.tipo.as_str()) {
            return Err(AppError::BadRequest(format!(
                "Red social desconocida: '{}'. Se admiten {}",
                r.tipo,
                TIPOS_RED_SOCIAL.join(", ")
            )));
        }
        if !vistos.insert(r.tipo.as_str()) {
            return Err(AppError::BadRequest(format!(
                "La red '{}' está repetida",
                r.tipo
            )));
        }
        if r.url.trim().is_empty() {
            return Err(AppError::BadRequest(format!(
                "Falta la dirección de {}",
                r.tipo
            )));
        }
        url_http(&r.tipo, &r.url)?;
    }
    Ok(())
}

/// Reemplaza los teléfonos del comercio. Si ninguno viene marcado como principal, lo es el primero.
async fn reemplazar_telefonos(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    empresa_id: i32,
    telefonos: &[TelefonoInput],
) -> AppResult<()> {
    sqlx::query("DELETE FROM telefono_empresa WHERE empresa_id = $1")
        .bind(empresa_id)
        .execute(&mut **tx)
        .await?;
    let principal = telefonos
        .iter()
        .position(|t| t.principal == Some(true))
        .unwrap_or(0);
    for (i, t) in telefonos.iter().enumerate() {
        sqlx::query(
            "INSERT INTO telefono_empresa (empresa_id, numero, descripcion, canal, es_whatsapp, principal) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(empresa_id)
        .bind(t.numero.trim())
        .bind(limpio(&t.descripcion))
        .bind(validar_modo_contacto(&t.canal)?)
        .bind(t.es_whatsapp.unwrap_or(false))
        .bind(i == principal)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn reemplazar_redes(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    empresa_id: i32,
    redes: &[RedSocial],
) -> AppResult<()> {
    sqlx::query("DELETE FROM red_social WHERE empresa_id = $1")
        .bind(empresa_id)
        .execute(&mut **tx)
        .await?;
    for r in redes {
        sqlx::query("INSERT INTO red_social (empresa_id, tipo, url) VALUES ($1, $2, $3)")
            .bind(empresa_id)
            .bind(&r.tipo)
            .bind(r.url.trim())
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn reemplazar_fotos(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    empresa_id: i32,
    fotos: &[String],
) -> AppResult<()> {
    sqlx::query("DELETE FROM foto_empresa WHERE empresa_id = $1")
        .bind(empresa_id)
        .execute(&mut **tx)
        .await?;
    for (orden, url) in fotos.iter().map(|f| f.trim()).filter(|f| !f.is_empty()).enumerate() {
        sqlx::query("INSERT INTO foto_empresa (empresa_id, url, orden) VALUES ($1, $2, $3)")
            .bind(empresa_id)
            .bind(url)
            .bind(orden as i16)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Teléfonos, redes, fotos y etiquetas (los que vengan), en la misma transacción que la ficha.
async fn guardar_contacto(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    empresa_id: i32,
    body: &EmpresaRequest,
    etiquetas: &Option<Vec<String>>,
) -> AppResult<()> {
    if let Some(telefonos) = &body.telefonos {
        reemplazar_telefonos(tx, empresa_id, telefonos).await?;
    }
    if let Some(redes) = &body.redes_sociales {
        reemplazar_redes(tx, empresa_id, redes).await?;
    }
    if let Some(fotos) = &body.fotos {
        reemplazar_fotos(tx, empresa_id, fotos).await?;
    }
    if let Some(lista) = etiquetas {
        reemplazar_etiquetas(tx, empresa_id, lista).await?;
    }
    Ok(())
}

pub async fn create_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<EmpresaRequest>,
) -> AppResult<(StatusCode, Json<Empresa>)> {
    // RF-01: solo una cuenta de comercio (admin) o de plataforma puede registrar un negocio.
    if user.rol != "admin" && user.rol != "superadmin" {
        return Err(AppError::sin_permisos_por("solo una cuenta de comercio puede registrar un negocio"));
    }
    // Un admin de negocio solo puede crear una empresa a su propio nombre; solo un superadmin
    // puede asignarla a otro usuario pasando usuario_id en el body.
    validar_ficha(&body)?;
    let usuario_id = if user.rol == "superadmin" && body.usuario_id.is_some() {
        body.usuario_id
    } else {
        Some(user.id)
    };
    // Solo el superadmin puede fijar el estado de verificación en el alta; el resto nace sin solicitar.
    let cuit = normalizar_cuit(&body.cuit)?;
    let estado = if user.rol == "superadmin" {
        validar_estado_verificacion(&body.verificado_estado)?.map(str::to_string)
    } else {
        None
    };
    let verificado = estado.as_deref() == Some("verificado");
    let etiquetas = normalizar_etiquetas(&body.etiquetas)?;
    let modo_contacto = validar_modo_contacto(&body.modo_contacto)?;

    let mut tx = state.db.begin().await?;
    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO empresa (nombre, direccion, slogan, duracion_turno_min, rubro_id, usuario_id, \
         turnos_habilitados, descripcion, cuit, url_cuit, verificado, verificado_estado, lat, lng, modo_contacto) \
         VALUES ($1,$2,$3,$4::int2,$5,$6,$7,$8,$9,$10,$11,COALESCE($12,'sin_solicitar'),$13::numeric,$14::numeric,COALESCE($15,'ambos')) RETURNING id",
    )
    .bind(body.nombre.trim())
    .bind(body.direccion.trim())
    .bind(limpio(&body.slogan))
    .bind(body.duracion_turno_min)
    .bind(body.rubro_id)
    .bind(usuario_id)
    .bind(body.turnos_habilitados.unwrap_or(true))
    .bind(limpio(&body.descripcion))
    .bind(cuit.as_deref())
    .bind(limpio(&body.url_cuit))
    .bind(verificado)
    .bind(estado.as_deref())
    .bind(body.lat)
    .bind(body.lng)
    .bind(modo_contacto)
    .fetch_one(&mut *tx)
    .await
    .map_err(fk_a_bad_request)?;
    guardar_contacto(&mut tx, id, &body, &etiquetas).await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(fetch_empresa(&state.db, id).await?),
    ))
}

/// Un rubro inexistente es error del cliente (400), no del servidor.
fn fk_a_bad_request(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => {
            AppError::BadRequest("El rubro indicado no existe".into())
        }
        _ => AppError::Db(e),
    }
}

pub(crate) async fn verificar_dueno(
    state: &AppState,
    user: &CurrentUser,
    empresa_id: i32,
) -> AppResult<()> {
    let dueno =
        sqlx::query_scalar::<_, Option<i32>>("SELECT usuario_id FROM empresa WHERE id = $1")
            .bind(empresa_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound("Empresa no encontrada".into()))?;

    if user.rol != "superadmin" && dueno != Some(user.id) {
        return Err(AppError::sin_permisos());
    }
    Ok(())
}

pub async fn update_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<EmpresaRequest>,
) -> AppResult<Json<Empresa>> {
    validar_ficha(&body)?;
    verificar_dueno(&state, &user, id).await?;

    // Solo un superadmin puede reasignar el dueño; para el resto, usuario_id no se toca. Los campos
    // opcionales ausentes (fotos, turnos_habilitados, descripcion, etiquetas) conservan su valor.
    let nuevo_dueno = if user.rol == "superadmin" {
        body.usuario_id
    } else {
        None
    };
    // El CUIT lo puede cargar el dueño; el estado solo lo cambia el superadmin (y sincroniza el sello).
    let cuit = normalizar_cuit(&body.cuit)?;
    let nuevo_estado = if user.rol == "superadmin" {
        validar_estado_verificacion(&body.verificado_estado)?.map(str::to_string)
    } else {
        None
    };
    let etiquetas = normalizar_etiquetas(&body.etiquetas)?;
    let modo_contacto = validar_modo_contacto(&body.modo_contacto)?;

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE empresa SET nombre=$1, direccion=$2, slogan=$3, duracion_turno_min=$4::int2, \
         rubro_id=$5, usuario_id=COALESCE($6, usuario_id), \
         turnos_habilitados=COALESCE($7, turnos_habilitados), \
         descripcion=COALESCE($8, descripcion), cuit=COALESCE($9, cuit), \
         url_cuit=COALESCE($10, url_cuit), \
         verificado_estado=COALESCE($11, verificado_estado), \
         motivo_rechazo=CASE WHEN $11 IS NULL THEN motivo_rechazo ELSE NULL END, \
         verificado=CASE WHEN $11 IS NULL THEN verificado ELSE ($11 = 'verificado') END, \
         modo_contacto=COALESCE($12, modo_contacto), \
         lat=COALESCE($13::numeric, lat), lng=COALESCE($14::numeric, lng) \
         WHERE id=$15",
    )
    .bind(body.nombre.trim())
    .bind(body.direccion.trim())
    .bind(limpio(&body.slogan))
    .bind(body.duracion_turno_min)
    .bind(body.rubro_id)
    .bind(nuevo_dueno)
    .bind(body.turnos_habilitados)
    .bind(body.descripcion.as_deref().map(str::trim))
    .bind(cuit.as_deref())
    .bind(limpio(&body.url_cuit))
    .bind(nuevo_estado.as_deref())
    .bind(modo_contacto)
    .bind(body.lat)
    .bind(body.lng)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(fk_a_bad_request)?;
    guardar_contacto(&mut tx, id, &body, &etiquetas).await?;
    tx.commit().await?;

    Ok(Json(fetch_empresa(&state.db, id).await?))
}

pub async fn delete_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    verificar_dueno(&state, &user, id).await?;
    let result = sqlx::query("DELETE FROM empresa WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    match result {
        Ok(r) if r.rows_affected() == 0 => Err(AppError::NotFound("Empresa no encontrada".into())),
        Ok(_) => Ok(Json(
            json!({ "message": "Empresa eliminada correctamente" }),
        )),
        // turno.empresa_id no tiene ON DELETE CASCADE a propósito: no se borran reservas ajenas en silencio.
        Err(sqlx::Error::Database(d)) if d.code().as_deref() == Some("23503") => Err(
            AppError::Conflict("La empresa tiene turnos registrados y no puede eliminarse".into()),
        ),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
pub struct TelefonoInput {
    numero: String,
    descripcion: Option<String>,
    canal: Option<String>,
    es_whatsapp: Option<bool>,
    /// Si ninguno viene marcado, el principal es el primero de la lista.
    principal: Option<bool>,
}

#[derive(Deserialize)]
pub struct TelefonosRequest {
    telefonos: Vec<TelefonoInput>,
}

/// Reemplaza todos los teléfonos del comercio (el principal incluido).
pub async fn update_telefonos_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<TelefonosRequest>,
) -> AppResult<Json<Vec<Telefono>>> {
    validar_telefonos(&body.telefonos)?;
    verificar_dueno(&state, &user, id).await?;

    let mut tx = state.db.begin().await?;
    reemplazar_telefonos(&mut tx, id, &body.telefonos).await?;
    tx.commit().await?;

    get_telefonos_by_empresa(State(state), Path(id)).await
}

// ------------------------------------------------------------------ turnos recibidos (panel del comercio)

#[derive(FromRow, Serialize)]
pub struct TurnoRecibido {
    id: i32,
    fecha: NaiveDate,
    hora: NaiveTime,
    estado: String,
    cliente_nombre: String,
    /// Canal de contacto: el correo de la cuenta o el que dejó la persona al agendar sin cuenta.
    cliente_email: Option<String>,
    /// 'online' (reserva del cliente) o 'mostrador' (lo cargó el comercio, sin cuenta).
    origen: String,
    nota: Option<String>,
    /// true si el comercio lo cargó a mano para alguien sin cuenta (origen 'mostrador').
    sin_cuenta: bool,
}

/// Turnos próximos de un comercio, para su dueño (o un superadmin). Los cancelados no se listan.
pub async fn get_turnos_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Vec<TurnoRecibido>>> {
    verificar_dueno(&state, &user, id).await?;
    let rows = sqlx::query_as::<_, TurnoRecibido>(
        "SELECT t.id, t.fecha, t.hora, et.nombre AS estado, \
                COALESCE(u.nombre, t.cliente_nombre)::text AS cliente_nombre, \
                COALESCE(u.email, t.cliente_email)::text AS cliente_email, \
                t.origen, t.nota, (t.origen = 'mostrador') AS sin_cuenta \
         FROM turno t JOIN estado_turno et ON et.id = t.estado_turno_id \
         LEFT JOIN usuario u ON u.id = t.usuario_id \
         WHERE t.empresa_id = $1 AND et.nombre <> 'cancelado' AND t.fecha >= $2 \
         ORDER BY t.fecha, t.hora",
    )
    .bind(id)
    .bind(crate::agenda::ahora_local().date())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

// ------------------------------------------------------------------ calificaciones (RF-13)

#[derive(Deserialize)]
pub struct CalificacionRequest {
    turno_id: i32,
    puntaje: i16,
    comentario: Option<String>,
}

pub async fn calificar_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<CalificacionRequest>,
) -> AppResult<Json<Value>> {
    if !(1..=5).contains(&body.puntaje) {
        return Err(AppError::BadRequest(
            "El puntaje debe estar entre 1 y 5".into(),
        ));
    }
    let comentario = body
        .comentario
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if comentario.is_some_and(|c| c.chars().count() > 1000) {
        return Err(AppError::BadRequest(
            "El comentario no puede superar los 1000 caracteres".into(),
        ));
    }
    let dueno = sqlx::query_scalar::<_, Option<i32>>("SELECT usuario_id FROM empresa WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("Empresa no encontrada".into()))?;
    if dueno == Some(user.id) {
        return Err(AppError::sin_permisos_por("no podés calificar tu propio comercio"));
    }

    // RF-13: la reseña es de un turno del propio cliente, ya realizado y no cancelado.
    let turno = sqlx::query_as::<_, (i32, NaiveDate, NaiveTime, String)>(
        "SELECT t.empresa_id, t.fecha, t.hora, et.nombre::text FROM turno t \
         JOIN estado_turno et ON et.id = t.estado_turno_id WHERE t.id = $1 AND t.usuario_id = $2",
    )
    .bind(body.turno_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("Turno no encontrado".into()))?;
    if turno.0 != id {
        return Err(AppError::BadRequest(
            "Ese turno pertenece a otro comercio".into(),
        ));
    }
    if !matches!(turno.3.as_str(), "confirmado" | "completado") {
        return Err(AppError::BadRequest(
            "No podés calificar un turno cancelado o no asistido".into(),
        ));
    }
    if turno.1.and_time(turno.2) > crate::agenda::ahora_local() {
        return Err(AppError::BadRequest(
            "Solo podés calificar después del horario del turno".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO calificacion (usuario_id, empresa_id, turno_id, puntaje, comentario) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (turno_id) WHERE turno_id IS NOT NULL \
         DO UPDATE SET puntaje = EXCLUDED.puntaje, comentario = EXCLUDED.comentario",
    )
    .bind(user.id)
    .bind(id)
    .bind(body.turno_id)
    .bind(body.puntaje)
    .bind(comentario)
    .execute(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => {
            AppError::NotFound("Turno o empresa no encontrada".into())
        }
        _ => AppError::Db(e),
    })?;
    Ok(Json(json!({ "message": "Calificación registrada" })))
}

#[derive(FromRow, Serialize)]
pub struct CalificacionPublica {
    usuario_nombre: String,
    puntaje: i16,
    comentario: Option<String>,
    creado_en: DateTime<Utc>,
}

pub async fn get_calificaciones(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Vec<CalificacionPublica>>> {
    let rows = sqlx::query_as::<_, CalificacionPublica>(
        "SELECT u.nombre AS usuario_nombre, k.puntaje, k.comentario, k.creado_en \
         FROM calificacion k JOIN usuario u ON u.id = k.usuario_id \
         WHERE k.empresa_id = $1 ORDER BY k.creado_en DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

// ------------------------------------------------------------------ mapa (RF-07)

#[derive(FromRow, Serialize)]
pub struct PuntoMapa {
    id: i32,
    nombre: String,
    lat: f64,
    lng: f64,
    rubro_id: i32,
    verificado: bool,
    slug: Option<String>,
}

/// Los mismos filtros que el listado, para que el mapa muestre lo que se buscó.
#[derive(Deserialize)]
pub struct MapaQuery {
    rubro_id: Option<i32>,
    q: Option<String>,
    etiquetas: Option<String>,
}

/// Solo los comercios geolocalizados y solo lo mínimo para dibujar los marcadores; la ficha
/// completa se pide al abrir cada uno.
pub async fn get_mapa(
    State(state): State<AppState>,
    Query(f): Query<MapaQuery>,
) -> AppResult<Json<Vec<PuntoMapa>>> {
    let rows = sqlx::query_as::<_, PuntoMapa>(&format!(
        "SELECT e.id, e.nombre, e.lat::float8 AS lat, e.lng::float8 AS lng, e.rubro_id, e.verificado, e.slug \
         FROM empresa e LEFT JOIN rubro r ON r.id = e.rubro_id \
         WHERE e.lat IS NOT NULL AND e.lng IS NOT NULL AND {} \
         {ORDEN_LISTADO}",
        filtro_directorio(1, 2, 3)
    ))
    .bind(f.q.as_deref().and_then(expandir_busqueda))
    .bind(f.rubro_id)
    .bind(etiquetas_filtro(&f.etiquetas))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_franjas_validas() {
        let (i, f) = parse_rango("09:00-13:30").unwrap();
        assert_eq!(
            (i.to_string(), f.to_string()),
            ("09:00:00".into(), "13:30:00".into())
        );
        assert!(parse_rango(" 08:15 - 12:00 ").is_ok());
    }

    #[test]
    fn rechaza_franjas_invalidas() {
        for malo in [
            "",
            "09:00",
            "09:00-",
            "-10:00",
            "25:00-26:00",
            "9-10",
            "10:00-09:00",
            "10:00-10:00",
            "ab-cd",
        ] {
            assert!(parse_rango(malo).is_err(), "{malo}");
        }
    }

    #[test]
    fn rechaza_franjas_superpuestas_del_mismo_dia() {
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        let partido = [("Lunes", t("16:00"), t("20:00")), ("Lunes", t("09:00"), t("13:00"))];
        assert!(validar_sin_superposicion(&partido).is_ok());
        let pegadas = [("Lunes", t("09:00"), t("13:00")), ("Lunes", t("13:00"), t("17:00"))];
        assert!(validar_sin_superposicion(&pegadas).is_ok(), "contiguas no se pisan");
        let otro_dia = [("Lunes", t("09:00"), t("13:00")), ("Martes", t("10:00"), t("12:00"))];
        assert!(validar_sin_superposicion(&otro_dia).is_ok());
        let pisadas = [("Lunes", t("09:00"), t("13:00")), ("Lunes", t("11:00"), t("15:00"))];
        assert!(validar_sin_superposicion(&pisadas).is_err());
        let repetidas = [("Sábado", t("09:00"), t("13:00")), ("Sábado", t("09:00"), t("13:00"))];
        assert!(validar_sin_superposicion(&repetidas).is_err());
    }

    #[test]
    fn el_listado_no_incluye_la_descripcion_pero_la_ficha_si() {
        assert!(read_select(false, false).contains("NULL::text AS descripcion"));
        assert!(read_select(true, false).contains("e.descripcion::text AS descripcion"));
        assert!(read_select(false, true).contains("AS distancia_km"));
        assert!(read_select(false, false).contains("AS etiquetas"));
        assert!(!read_select(false, false).contains("e.lat::float8 - $8"));
    }

    #[test]
    fn normaliza_alias_y_deduplica_etiquetas() {
        let entrada = Some(vec![
            "wifi".into(),
            "Wi-Fi gratis".into(),
            "Acceso para sillas de ruedas".into(),
            " Terraza ".into(),
            "".into(),
        ]);
        assert_eq!(
            normalizar_etiquetas(&entrada).unwrap().unwrap(),
            ["Wi-Fi gratis", "Rampa de acceso", "Terraza"]
        );
        assert_eq!(normalizar_etiquetas(&None).unwrap(), None);
        assert_eq!(
            normalizar_etiquetas(&Some(Vec::new())).unwrap(),
            Some(Vec::new()),
            "una lista vacía borra las etiquetas"
        );
    }

    #[test]
    fn filtra_etiquetas_por_alias_sin_repetir() {
        let pedidas = Some("rampa,wifi,Wi-Fi gratis".to_string());
        assert_eq!(
            etiquetas_filtro(&pedidas),
            Some(vec!["Rampa de acceso".into(), "Wi-Fi gratis".into()])
        );
        assert_eq!(etiquetas_filtro(&Some(" , ".into())), None);
    }

    #[test]
    fn rechaza_demasiadas_etiquetas_o_nombres_largos() {
        let muchas = Some(vec!["Terraza".to_string(); MAX_ETIQUETAS + 1]);
        assert!(normalizar_etiquetas(&muchas).is_err());
        let larga = Some(vec!["x".repeat(MAX_LARGO_ETIQUETA + 1)]);
        assert!(normalizar_etiquetas(&larga).is_err());
    }
}
