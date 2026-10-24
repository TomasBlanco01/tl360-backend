// Sonda de disponibilidad para cron o un monitor externo (UptimeRobot, Better Stack, etc.).
// Uso: HEALTH_URL=https://api.ejemplo.com/health npm run uptime
const url = process.env.HEALTH_URL ?? "http://localhost:3001/health";
const timeoutMs = Number(process.env.HEALTH_TIMEOUT_MS ?? 5000);
const inicio = Date.now();

try {
	const res = await fetch(url, {
		cache: "no-store",
		signal: AbortSignal.timeout(timeoutMs)
	});
	const ms = Date.now() - inicio;
	const cuerpo = await res.text();
	if (!res.ok) {
		console.error(`DOWN ${url} -> HTTP ${res.status} en ${ms} ms: ${cuerpo.slice(0, 200)}`);
		process.exit(1);
	}
	console.log(`UP ${url} -> HTTP ${res.status} en ${ms} ms`);
} catch (error) {
	const ms = Date.now() - inicio;
	console.error(`DOWN ${url} -> ${error.message} en ${ms} ms`);
	process.exit(1);
}
