// Postgres local para desarrollo, sin Docker ni servicios externos.
//
//   npm run db        (desde tl360-backend/tools)   →   postgres://postgres:postgres@127.0.0.1:5433/tl360
//
// Qué hace, en orden (y se puede correr las veces que haga falta):
//   1. Inicia PostgreSQL en el puerto 5433 con los datos en tools/.pgdata (se conservan entre
//      ejecuciones). Si la carpeta no tiene un clúster inicializado, lo crea. Si ya hay un Postgres
//      escuchando en 5433 (por ejemplo, levantado con pg_ctl), usa ese y no lo detiene al salir.
//   2. Se conecta a la base administrativa `postgres`, consulta pg_database y crea `tl360` solo si
//      no existe. Cierra esa conexión y recién después se conecta a `tl360`.
//   3. Espera a que el backend (`cargo run`) aplique TODAS las migraciones de ../migrations: el
//      esquema lo crea sqlx::migrate, no este script.
//   4. Verifica que el esquema y los seeds sean del modelo v2 (tablas nuevas presentes, tablas
//      viejas ausentes, ningún seed que use nombres del modelo anterior).
//   5. Si la base no tiene datos, carga los seeds en una sola transacción: 01_rubros,
//      02_empresas (o 02_empresas_demo si el primero no está) y 03_simulados. Si algo falla, no queda
//      nada a medias.
//   6. Queda corriendo; Ctrl+C cierra la conexión y detiene el Postgres que inició.
import EmbeddedPostgres from "embedded-postgres";
import fs from "node:fs";
import path from "node:path";
import pg from "pg";
import { fileURLToPath } from "node:url";

// DEVDB_PUERTO y DEVDB_BASE son opcionales (por ejemplo, para probar una base nueva sin tocar tl360).
const PUERTO = Number(process.env.DEVDB_PUERTO ?? 5433);
const USUARIO = "postgres";
const CLAVE = "postgres";
const BASE = process.env.DEVDB_BASE ?? "tl360";
if (!/^[a-z_][a-z0-9_]*$/.test(BASE)) {
  console.error(`[devdb] ERROR: nombre de base inválido: ${BASE}`);
  process.exit(1);
}
const url = (base) => `postgres://${USUARIO}:${CLAVE}@127.0.0.1:${PUERTO}/${base}`;

const aqui = path.dirname(fileURLToPath(import.meta.url));
const DIR_DATOS = path.join(aqui, ".pgdata");
const DIR_MIGRACIONES = path.join(aqui, "..", "migrations");
const DIR_SEEDS = path.join(aqui, "..", "db", "seed");

// Tablas del modelo v2 que usan los seeds y tablas del modelo anterior que ya no deben existir.
const TABLAS_V2 = [
  "usuario", "rubro", "empresa", "telefono_empresa", "red_social", "foto_empresa", "horario_empresa",
  "etiqueta", "empresa_etiqueta", "turno", "estado_turno", "calificacion", "favorito", "reporte",
  "publicidad", "difusion", "estado_aprobacion",
];
const TABLAS_VIEJAS = [
  "usuarios", "empresas", "categorias", "super_rubros", "empresa_categorias", "turnos",
  "atenciones_mostrador", "resumenes_semanales", "solicitudes_publicidad", "publicaciones_destacadas",
  "telefonos_empresa", "horarios_empresa", "calificaciones", "favoritos", "reportes",
];
// Lo mismo en el SQL de los seeds: tablas usadas después de INSERT INTO/UPDATE/FROM/JOIN (así no
// se confunde con la palabra "empresas" dentro de una descripción) y columnas que ya no existen.
const NOMBRES_VIEJOS = new RegExp(
  `\\b(?:INSERT\\s+INTO|UPDATE|FROM|JOIN)\\s+(${TABLAS_VIEJAS.join("|")})\\b` +
    "|\\b(admin_id|categoria_id|constancia_cuit_url|imagen_url|super_rubro_id)\\b",
  "i",
);

let servidor = null; // solo si lo inició este script
let cliente = null;

const log = (...m) => console.log("[devdb]", ...m);
class ErrorDevdb extends Error {}

async function conectar(base, { silencioso = false } = {}) {
  const c = new pg.Client({ connectionString: url(base), connectionTimeoutMillis: 3000 });
  try {
    await c.connect();
    return c;
  } catch (e) {
    await c.end().catch(() => {});
    if (silencioso) return null;
    throw e;
  }
}

/** 1. Usa el Postgres que ya escucha en el puerto o inicia el embebido. */
async function iniciarPostgres() {
  const existente = await conectar("postgres", { silencioso: true });
  if (existente) {
    await existente.end();
    log(`ya hay un Postgres escuchando en ${PUERTO}: se usa ese (no se detendrá al salir)`);
    return;
  }

  const inicializado = fs.existsSync(path.join(DIR_DATOS, "PG_VERSION"));
  if (!inicializado && fs.existsSync(DIR_DATOS) && fs.readdirSync(DIR_DATOS).length > 0) {
    throw new ErrorDevdb(
      `${DIR_DATOS} existe pero no contiene un clúster de Postgres (falta PG_VERSION).\n` +
        "Si no tiene datos que te importen, borrala y volvé a correr npm run db.",
    );
  }

  servidor = new EmbeddedPostgres({
    databaseDir: DIR_DATOS,
    user: USUARIO,
    password: CLAVE,
    port: PUERTO,
    persistent: true,
    initdbFlags: ["--encoding=UTF8", "--locale=C"],
    onLog: () => {},
    onError: (m) => console.error("[postgres]", String(m).trim()),
  });

  try {
    if (!inicializado) {
      log(`inicializando un clúster nuevo en ${DIR_DATOS}`);
      await servidor.initialise();
    }
    await servidor.start();
  } catch (e) {
    servidor = null;
    const texto = String(e?.message ?? e);
    if (/administrative permissions|root/i.test(texto)) {
      throw new ErrorDevdb(
        "Postgres no se ejecuta con una cuenta de administrador.\n" +
          "Abrí la terminal sin \"Ejecutar como administrador\", o levantalo con pg_ctl (baja los permisos) y volvé a correr npm run db:\n" +
          `  node_modules/@embedded-postgres/windows-x64/native/bin/pg_ctl.exe -D .pgdata -o "-p ${PUERTO}" -l devdb_pgctl.log start -w`,
      );
    }
    throw new ErrorDevdb(`No se pudo iniciar Postgres en el puerto ${PUERTO}: ${texto}`);
  }
  log(`Postgres iniciado en el puerto ${PUERTO} (datos en ${DIR_DATOS})`);
}

/** 2. Crea la base de la aplicación si no existe, desde la base administrativa `postgres`. */
async function asegurarBase() {
  const admin = await conectar("postgres");
  try {
    const { rowCount } = await admin.query("SELECT 1 FROM pg_database WHERE datname = $1", [BASE]);
    if (rowCount > 0) {
      log(`la base ${BASE} ya existe`);
      return;
    }
    try {
      await admin.query(`CREATE DATABASE ${BASE} ENCODING 'UTF8' TEMPLATE template0`);
      log(`base ${BASE} creada`);
    } catch (e) {
      // Otro proceso la creó entre la consulta y el CREATE: no es un error.
      if (e.code !== "42P04") throw e;
    }
  } finally {
    await admin.end();
  }
}

/** Versiones de las migraciones del repositorio (el número antes del primer "_"). */
function migracionesDelRepo() {
  return fs
    .readdirSync(DIR_MIGRACIONES)
    .filter((f) => /^\d+_.*\.sql$/.test(f))
    .map((f) => Number(f.split("_")[0]))
    .sort((a, b) => a - b);
}

/** 3. Espera a que sqlx haya aplicado todas las migraciones del repositorio. */
async function esperarMigraciones() {
  const esperadas = migracionesDelRepo();
  let ultimoAviso = 0;
  for (;;) {
    const tabla = await cliente.query("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL AS hay");
    if (tabla.rows[0].hay) {
      const { rows } = await cliente.query("SELECT version, success FROM _sqlx_migrations");
      const fallida = rows.find((r) => !r.success);
      if (fallida) {
        throw new ErrorDevdb(
          `la migración ${fallida.version} quedó marcada como fallida en _sqlx_migrations; revisá el log del backend.`,
        );
      }
      const aplicadas = new Set(rows.map((r) => Number(r.version)));
      const faltan = esperadas.filter((v) => !aplicadas.has(v));
      if (faltan.length === 0) {
        log(`migraciones al día (${esperadas.length})`);
        return;
      }
      if (Date.now() - ultimoAviso > 15000) {
        log(`faltan aplicar las migraciones ${faltan.join(", ")}: levantá el backend (cargo run) y se aplican solas`);
        ultimoAviso = Date.now();
      }
    } else if (Date.now() - ultimoAviso > 15000) {
      log("la base está vacía: levantá el backend (cargo run en tl360-backend) para que cree las tablas");
      ultimoAviso = Date.now();
    }
    await new Promise((r) => setTimeout(r, 2000));
  }
}

/** Seeds a cargar: 02_empresas.sql (importados, no versionado) o los de demostración.
 *  DEVDB_SEED=demo fuerza los de demostración aunque exista 02_empresas.sql. */
function seedsACargar() {
  const importados =
    process.env.DEVDB_SEED !== "demo" && fs.existsSync(path.join(DIR_SEEDS, "02_empresas.sql"));
  return ["01_rubros", importados ? "02_empresas" : "02_empresas_demo", "03_simulados"].map((n) => ({
    nombre: n,
    ruta: path.join(DIR_SEEDS, `${n}.sql`),
  }));
}

/** 4. El esquema y los seeds tienen que ser del modelo v2. */
async function verificarModelo(seeds) {
  const { rows } = await cliente.query(
    "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
  );
  const tablas = new Set(rows.map((r) => r.table_name));
  const faltan = TABLAS_V2.filter((t) => !tablas.has(t));
  if (faltan.length) {
    throw new ErrorDevdb(`al esquema le faltan tablas del modelo v2: ${faltan.join(", ")}`);
  }
  const viejas = TABLAS_VIEJAS.filter((t) => tablas.has(t));
  if (viejas.length) {
    throw new ErrorDevdb(`el esquema todavía tiene tablas del modelo anterior: ${viejas.join(", ")}`);
  }
  for (const s of seeds) {
    if (!fs.existsSync(s.ruta)) throw new ErrorDevdb(`no se encontró el seed ${s.ruta}`);
    const sql = fs.readFileSync(s.ruta, "utf8");
    const lineas = sql.split("\n");
    const i = lineas.findIndex((l) => !l.trimStart().startsWith("--") && NOMBRES_VIEJOS.test(l));
    if (i >= 0) {
      const m = lineas[i].match(NOMBRES_VIEJOS);
      const nombre = m[1] ?? m[2];
      throw new ErrorDevdb(`${s.nombre}.sql:${i + 1} usa '${nombre}', que no existe en el modelo v2`);
    }
  }
  log("esquema y seeds compatibles con el modelo v2");
}

/** 5. Carga los seeds en una transacción, solo si la base todavía no tiene datos. */
async function cargarSeeds(seeds) {
  const { rows } = await cliente.query("SELECT (SELECT count(*) FROM rubro) + (SELECT count(*) FROM empresa) AS n");
  if (Number(rows[0].n) > 0) {
    log("la base ya tiene datos: no se cargan los seeds (para empezar de cero, ver README)");
    return;
  }
  await cliente.query("BEGIN");
  for (const s of seeds) {
    try {
      await cliente.query(fs.readFileSync(s.ruta, "utf8"));
      log(`seed cargado: ${s.nombre}`);
    } catch (e) {
      await cliente.query("ROLLBACK").catch(() => {});
      const donde = e.position ? ` (posición ${e.position})` : "";
      throw new ErrorDevdb(`el seed ${s.nombre}.sql falló${donde}: ${e.message}. No se cargó ningún seed.`);
    }
  }
  await cliente.query("COMMIT");
  const r = await cliente.query(
    "SELECT (SELECT count(*) FROM rubro) AS rubros, (SELECT count(*) FROM empresa) AS comercios, (SELECT count(*) FROM usuario) AS usuarios",
  );
  log(`seeds cargados: ${r.rows[0].rubros} rubros, ${r.rows[0].comercios} comercios, ${r.rows[0].usuarios} usuarios`);
}

async function cerrar(codigo) {
  if (cliente) await cliente.end().catch(() => {});
  cliente = null;
  if (servidor) {
    log("deteniendo Postgres...");
    await servidor.stop().catch((e) => console.error("[devdb] no se pudo detener Postgres:", e?.message ?? e));
    servidor = null;
  }
  process.exit(codigo);
}

process.on("SIGINT", () => cerrar(0));
process.on("SIGTERM", () => cerrar(0));

try {
  await iniciarPostgres();
  await asegurarBase();
  cliente = await conectar(BASE);
  // Si la conexión se corta (por ejemplo, alguien detuvo Postgres), se informa en vez de colgarse.
  cliente.on("error", (e) => {
    console.error("[devdb] se perdió la conexión con Postgres:", e.message);
    cerrar(1);
  });
  log(`listo: ${url(BASE)}`);
  await esperarMigraciones();
  const seeds = seedsACargar();
  await verificarModelo(seeds);
  await cargarSeeds(seeds);
  console.log("SEED-LISTO");
  if (servidor) {
    log("Postgres sigue corriendo; Ctrl+C para detenerlo");
    await cliente.end();
    cliente = null;
  } else {
    await cerrar(0);
  }
} catch (e) {
  console.error(`[devdb] ERROR: ${e instanceof ErrorDevdb ? e.message : (e?.stack ?? e)}`);
  await cerrar(1);
}
