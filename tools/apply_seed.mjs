// Aplica un seed SQL a la base local:  node apply_seed.mjs ../db/seed/04_geo.sql
import pg from 'pg';
import fs from 'node:fs';
const archivo = process.argv[2];
if (!archivo) { console.error('uso: node apply_seed.mjs <archivo.sql>'); process.exit(1); }
const c = new pg.Client(process.env.DATABASE_URL ?? 'postgres://postgres:postgres@127.0.0.1:5433/tl360');
await c.connect();
await c.query(fs.readFileSync(archivo, 'utf8'));
const r = await c.query('select count(*) total, count(lat) con_coordenadas from empresas');
console.log('empresas:', r.rows[0]);
await c.end();
