// Valida las migraciones (todas, en orden) y los seeds sobre PGlite (Postgres en WASM), sin servidor.
import { PGlite } from "@electric-sql/pglite";
import { pg_trgm } from "@electric-sql/pglite/contrib/pg_trgm";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const raiz = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const db = new PGlite({ extensions: { pg_trgm } });
const run = async (f) => { await db.exec(fs.readFileSync(path.join(raiz, f), "utf8")); console.log("OK", f); };

const migraciones = fs.readdirSync(path.join(raiz, "migrations")).filter((f) => f.endsWith(".sql")).sort();
for (const m of migraciones) await run(`migrations/${m}`);
for (const s of ["01_rubros", "02_empresas", "03_simulados"]) await run(`db/seed/${s}.sql`);

const q = async (s) => (await db.query(s)).rows;
for (const t of ["usuarios", "categorias", "empresas", "empresa_categorias", "telefonos_empresa", "horarios_empresa", "calificaciones", "etiquetas", "empresa_etiquetas", "publicaciones_destacadas", "solicitudes_publicidad", "atenciones_mostrador", "turnos"])
  console.log(t.padEnd(26), (await q(`select count(*)::int n from ${t}`))[0].n);
console.log("empresas sin horarios:", (await q("select count(*)::int n from empresas e where not exists (select 1 from horarios_empresa h where h.empresa_id=e.id)"))[0].n);

// RF-03 + RF-05: un solo turno activo por horario; cancelado libera el horario
const emp = (await q("select id from empresas where turnos_habilitados limit 1"))[0].id;
const usr = (await q("select id from usuarios where email = 'cliente@demo.com'"))[0].id;
const ins = () => db.query(
  "insert into turnos (empresa_id,usuario_id,fecha,hora) values ($1,$2,'2030-01-08','10:00') on conflict (empresa_id,fecha,hora) where estado <> 'cancelado' do nothing returning id", [emp, usr]);
const a = await ins(), b = await ins();
console.log("primer turno:", a.rows.length, "· segundo mismo horario (debe ser 0):", b.rows.length);
await db.query("update turnos set estado = 'cancelado' where id = $1", [a.rows[0].id]);
console.log("tras cancelar, se puede reservar de nuevo (debe ser 1):", (await ins()).rows.length);
