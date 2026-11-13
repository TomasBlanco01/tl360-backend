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
// 02_empresas.sql (comercios importados) no se versiona; en un clon limpio se usan los de demostración.
const comercios = fs.existsSync(path.join(raiz, "db/seed/02_empresas.sql")) ? "02_empresas" : "02_empresas_demo";
for (const s of ["01_rubros", comercios, "03_simulados"]) await run(`db/seed/${s}.sql`);

const q = async (s) => (await db.query(s)).rows;
for (const t of ["usuario", "rubro", "empresa", "telefono_empresa", "red_social", "foto_empresa", "horario_empresa", "calificacion", "etiqueta", "empresa_etiqueta", "publicidad", "difusion", "turno"])
  console.log(t.padEnd(26), (await q(`select count(*)::int n from ${t}`))[0].n);
console.log("empresas sin horarios:", (await q("select count(*)::int n from empresa e where not exists (select 1 from horario_empresa h where h.empresa_id=e.id)"))[0].n);

// RF-03 + RF-05: un solo turno activo por horario; cancelado libera el horario
const emp = (await q("select id from empresa where turnos_habilitados limit 1"))[0].id;
const usr = (await q("select id from usuario where email = 'cliente@demo.com'"))[0].id;
const ins = () => db.query(
  "insert into turno (empresa_id,usuario_id,fecha,hora) values ($1,$2,'2030-01-08','10:00') on conflict (empresa_id,fecha,hora) where estado_turno_id <> 2 do nothing returning id", [emp, usr]);
const a = await ins(), b = await ins();
console.log("primer turno:", a.rows.length, "· segundo mismo horario (debe ser 0):", b.rows.length);
await db.query("update turno set estado_turno_id = 2 where id = $1", [a.rows[0].id]);
console.log("tras cancelar, se puede reservar de nuevo (debe ser 1):", (await ins()).rows.length);
