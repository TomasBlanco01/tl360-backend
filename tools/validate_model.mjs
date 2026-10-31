import { PGlite } from "@electric-sql/pglite";
import { pg_trgm } from "@electric-sql/pglite/contrib/pg_trgm";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Cuenta tablas, columnas e índices del esquema aplicando las migraciones (la fuente de verdad).
const migraciones = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "migrations");
const db = new PGlite({ extensions: { pg_trgm } });
try {
  for (const m of fs.readdirSync(migraciones).filter((f) => f.endsWith(".sql")).sort())
    await db.exec(fs.readFileSync(path.join(migraciones, m), "utf8"));
  const cols = (await db.query(`select count(*)::int n from information_schema.columns where table_schema='public'`)).rows[0].n;
  const tabs = (await db.query(`select count(*)::int n from information_schema.tables where table_schema='public'`)).rows[0].n;
  const idx = (await db.query(`select count(*)::int n from pg_indexes where schemaname='public'`)).rows[0].n;
  const cons = (await db.query(`select count(*)::int n from information_schema.table_constraints where table_schema='public'`)).rows[0].n;
  console.log("OK DDL — tablas:", tabs, "columnas:", cols, "índices:", idx, "constraints:", cons);
  const trig = (await db.query(`select trigger_name from information_schema.triggers where trigger_schema='public'`)).rows;
  console.log("triggers:", trig.map(r=>r.trigger_name).join(", "));
} catch (e) {
  console.error("ERROR:", e.message);
  process.exit(1);
}
