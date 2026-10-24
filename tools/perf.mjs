// Benchmark del camino crítico de RNF-01: listado de comercios y ficha.
// Requiere el backend levantado: PERF_URL=http://localhost:3001 npm run perf
const host = (process.env.PERF_URL ?? "http://localhost:3001").replace(/\/$/, "");
const presupuestoMs = Number(process.env.PERF_BUDGET_MS ?? 2500);
const repeticiones = Number(process.env.PERF_REQUESTS ?? 30);
const calentamiento = Number(process.env.PERF_WARMUP ?? 3);

async function pedir(path) {
	const inicio = performance.now();
	const res = await fetch(host + path, { headers: { Accept: "application/json" } });
	const cuerpo = await res.text();
	const ms = performance.now() - inicio;
	if (!res.ok) throw new Error(`${path} -> HTTP ${res.status}: ${cuerpo.slice(0, 200)}`);
	return { ms, datos: JSON.parse(cuerpo) };
}

async function medir(nombre, path) {
	for (let i = 0; i < calentamiento; i++) await pedir(path);
	const tiempos = [];
	for (let i = 0; i < repeticiones; i++) tiempos.push((await pedir(path)).ms);
	tiempos.sort((a, b) => a - b);
	const p50 = tiempos[Math.floor(repeticiones * 0.5)];
	const p95 = tiempos[Math.max(0, Math.ceil(repeticiones * 0.95) - 1)];
	const max = tiempos[tiempos.length - 1];
	return {
		nombre,
		p50: Math.round(p50),
		p95: Math.round(p95),
		max: Math.round(max),
		ok: p95 < presupuestoMs
	};
}

const inicial = await pedir("/api/empresas?limit=1");
const empresaId = inicial.datos?.[0]?.id;
if (!empresaId) throw new Error("el listado no devolvió ningún comercio para medir la ficha");

const resultados = [];
resultados.push(await medir("Listado (25 comercios)", "/api/empresas?limit=25"));
resultados.push(await medir(`Ficha (id ${empresaId})`, `/api/empresas/${empresaId}`));

console.log(`RNF-01: presupuesto p95 < ${presupuestoMs} ms (${host}, n=${repeticiones})`);
console.table(resultados);
if (resultados.some((r) => !r.ok)) {
	console.error("RNF-01 FALLA: algún camino crítico superó el presupuesto.");
	process.exit(1);
}
console.log("RNF-01 OK: listado y ficha por debajo del presupuesto.");
