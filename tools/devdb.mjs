// Postgres real y embebido para desarrollo local (sin Docker ni Supabase).
// Uso: node devdb.mjs   → deja la base en postgres://postgres:postgres@127.0.0.1:5433/tl360
// El backend crea el esquema con sqlx::migrate; este script carga los seeds cuando lo detecta
// (solo la primera vez: los datos persisten en tools/.pgdata).
import EmbeddedPostgres from "embedded-postgres";
import fs from "node:fs";
import pg from "pg";
import { fileURLToPath } from "node:url";

const dir = fileURLToPath(new URL("./.pgdata", import.meta.url));
const primera = !fs.existsSync(dir);
const server = new EmbeddedPostgres({ databaseDir: dir, user: "postgres", password: "postgres", port: 5433, persistent: true, initdbFlags: ["--encoding=UTF8", "--locale=C"] });
if (primera) await server.initialise();
await server.start();
if (primera) await server.createDatabase("tl360");
console.log("devdb listo: postgres://postgres:postgres@127.0.0.1:5433/tl360");

const cliente = new pg.Client({ connectionString: "postgres://postgres:postgres@127.0.0.1:5433/tl360" });
await cliente.connect();
let cargado = false;
setInterval(async () => {
  if (cargado) return;
  try {
    const ya = (await cliente.query("select to_regclass('public.turnos') is not null as ok, to_regclass('public.super_rubros') is not null as sr")).rows[0];
    if (!ya.ok) return;
    const hay = (await cliente.query("select count(*)::int n from categorias")).rows[0].n;
    cargado = true;
    if (hay > 0) return console.log("seeds ya cargados");
    for (const f of ["01_rubros", "02_empresas", "03_simulados"]) {
      await cliente.query(fs.readFileSync(fileURLToPath(new URL(`../db/seed/${f}.sql`, import.meta.url)), "utf8"));
      console.log("seed cargado:", f);
    }
    console.log("SEED-LISTO");
  } catch (e) { cargado = false; }
}, 1000);

process.on("SIGINT", async () => { await cliente.end(); await server.stop(); process.exit(0); });
