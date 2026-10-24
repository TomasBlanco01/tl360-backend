// Genera un par VAPID P-256 para Web Push sin depender de OpenSSL ni de paquetes externos.
// Uso: node tools/vapid.mjs
// Guardá VAPID_PRIVATE_KEY como secreto del servidor; VAPID_PUBLIC_KEY es solo para verificación.
import { generateKeyPairSync } from 'node:crypto';

const { privateKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
const jwk = privateKey.export({ format: 'jwk' });
const privada = Buffer.from(jwk.d, 'base64url');
const publica = Buffer.concat([
	Buffer.from([4]),
	Buffer.from(jwk.x, 'base64url'),
	Buffer.from(jwk.y, 'base64url')
]);

console.log(`VAPID_PRIVATE_KEY=${privada.toString('base64url')}`);
console.log(`VAPID_PUBLIC_KEY=${publica.toString('base64url')}`);
console.log('VAPID_SUBJECT=mailto:admin@tudominio.com');
