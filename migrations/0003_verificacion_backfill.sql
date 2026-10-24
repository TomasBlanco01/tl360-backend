-- RF-01: la verificación de email pasa a ser obligatoria para reservar/cancelar turnos.
-- Las cuentas creadas antes de este cambio quedan marcadas como verificadas para no
-- bloquearlas de golpe; las nuevas nacen sin verificar (ver handlers/auth.rs).
UPDATE usuarios SET email_verificado = true WHERE email_verificado = false;
