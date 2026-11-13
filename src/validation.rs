use crate::errors::{AppError, AppResult};

/// bcrypt solo usa los primeros 72 bytes de la contraseña; se limita para no aceptar claves que
/// en realidad se truncan en silencio ni pagar el costo de hashear entradas gigantes.
/// Misma regla que muestra el formulario (registro, recuperación, Mis datos y alta del superadmin):
/// al menos 8 caracteres, con mayúscula, minúscula y número.
pub fn password_valida(p: &str) -> AppResult<()> {
    let segura = p.chars().count() >= 8
        && p.chars().any(char::is_lowercase)
        && p.chars().any(char::is_uppercase)
        && p.chars().any(|c| c.is_ascii_digit());
    if !segura {
        return Err(AppError::BadRequest(
            "La contraseña debe tener al menos 8 caracteres e incluir mayúscula, minúscula y número"
                .into(),
        ));
    }
    if p.len() > 72 {
        return Err(AppError::BadRequest(
            "La contraseña no puede superar los 72 caracteres".into(),
        ));
    }
    Ok(())
}

/// Formato de correo usual (RFC 5321 sin comillas ni IP literales):
/// - parte local de hasta 64 caracteres, con letras, números y `.!#$%&'*+/=?^_`{|}~-`, sin puntos
///   al principio, al final ni seguidos;
/// - dominio con al menos dos etiquetas de letras, números y guiones (sin guion en los bordes) y
///   una terminación de al menos 2 letras.
pub fn email_valido(e: &str) -> AppResult<()> {
    let e = e.trim();
    let mut partes = e.split('@');
    let (local, dominio) = (partes.next().unwrap_or(""), partes.next().unwrap_or(""));
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._%+-!#$&'*/=?^`{|}~".contains(c));
    let etiquetas: Vec<&str> = dominio.split('.').collect();
    let dominio_ok = etiquetas.len() >= 2
        && etiquetas.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        && etiquetas
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()));
    let ok = e.len() <= 254 && partes.next().is_none() && local_ok && dominio_ok;
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "El correo electrónico no es válido".into(),
        ))
    }
}

/// Teléfono: dígitos con los separadores habituales (espacio, guion, punto, paréntesis y un `+`
/// inicial), entre 6 y 15 dígitos (máximo de E.164) y hasta 20 caracteres (VARCHAR(20)).
pub fn telefono_valido(t: &str) -> AppResult<()> {
    let t = t.trim();
    let digitos = t.chars().filter(char::is_ascii_digit).count();
    let simbolos_ok = t.chars().enumerate().all(|(i, c)| {
        c.is_ascii_digit() || " -.()".contains(c) || (c == '+' && i == 0)
    });
    if simbolos_ok && (6..=15).contains(&digitos) && t.chars().count() <= 20 {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "El teléfono '{t}' no es válido: usá solo números (de 6 a 15 dígitos)"
        )))
    }
}

pub fn nombre_valido(n: &str) -> AppResult<()> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 100 {
        return Err(AppError::BadRequest(
            "El nombre es obligatorio (hasta 100 caracteres)".into(),
        ));
    }
    Ok(())
}

/// Las URLs guardadas terminan como `href`/`src` en el front: solo se aceptan http(s) para no
/// permitir esquemas como `javascript:`.
pub fn url_http(campo: &str, valor: &str) -> AppResult<()> {
    let v = valor.trim();
    if v.is_empty() {
        return Ok(());
    }
    let lower = v.to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://"))
        && v.len() <= 2048
        && !v.chars().any(char::is_whitespace)
    {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "{campo} debe ser una URL http(s) válida"
        )))
    }
}

pub fn url_http_opt(campo: &str, valor: &Option<String>) -> AppResult<()> {
    valor.as_deref().map_or(Ok(()), |v| url_http(campo, v))
}

/// Largo máximo de un texto, en caracteres: es el mismo N de la columna VARCHAR(N), así un valor
/// largo se rechaza con un mensaje claro en vez de un error de la base.
pub fn texto_hasta(campo: &str, valor: &str, max: usize) -> AppResult<()> {
    if valor.trim().chars().count() > max {
        return Err(AppError::BadRequest(format!(
            "{campo} no puede superar los {max} caracteres"
        )));
    }
    Ok(())
}

pub fn texto_hasta_opt(campo: &str, valor: &Option<String>, max: usize) -> AppResult<()> {
    valor.as_deref().map_or(Ok(()), |v| texto_hasta(campo, v, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_validos() {
        for e in ["a@b.com", "nombre.apellido@dominio.com.ar", "  x@y.io  "] {
            assert!(email_valido(e).is_ok(), "{e}");
        }
    }

    #[test]
    fn emails_invalidos() {
        for e in [
            "",
            "sin-arroba",
            "@dominio.com",
            "a@",
            "a@b",
            "a@@b.com",
            "a b@c.com",
            "a@.com",
            "a@b.",
            "a@b@c.com",
            "a..b@c.com",
            ".a@c.com",
            "a.@c.com",
            "a,b@c.com",
            "<a>@c.com",
            "a@b..com",
            "a@-b.com",
            "a@b-.com",
            "a@b.c",
            "a@b.c0m",
            "a@dominio_con_guion_bajo.com",
        ] {
            assert!(email_valido(e).is_err(), "{e}");
        }
        assert!(email_valido(&format!("{}@x.com", "a".repeat(260))).is_err());
        assert!(email_valido(&format!("{}@x.com", "a".repeat(65))).is_err(), "parte local > 64");
    }

    #[test]
    fn emails_validos_poco_comunes() {
        for e in ["ana+turnos@gmail.com", "o'brien@mail.ie", "a-b_c@sub.dominio-ok.com.ar"] {
            assert!(email_valido(e).is_ok(), "{e}");
        }
    }

    #[test]
    fn telefonos() {
        for t in ["2392411234", "+54 9 2392 41-1234", "(02392) 411234", "02392.411234"] {
            assert!(telefono_valido(t).is_ok(), "{t}");
        }
        for t in ["", "12345", "abc123456", "2392-41-12a4", "54+2392411234", "1234567890123456"] {
            assert!(telefono_valido(t).is_err(), "{t}");
        }
        assert!(telefono_valido("+54 (9) 2392 411-23 4").is_err(), "más de 20 caracteres");
    }

    #[test]
    fn contrasenas() {
        assert!(password_valida("Clave1").is_err(), "corta");
        assert!(password_valida("clave1234").is_err(), "sin mayúscula");
        assert!(password_valida("CLAVE1234").is_err(), "sin minúscula");
        assert!(password_valida("ClaveSegura").is_err(), "sin número");
        assert!(password_valida("Clave1234").is_ok());
        assert!(password_valida(&format!("Aa1{}", "x".repeat(69))).is_ok());
        assert!(password_valida(&format!("Aa1{}", "x".repeat(70))).is_err(), "más de 72 bytes");
        // se cuentan caracteres para el mínimo, no bytes
        assert!(password_valida("Ññ1ñ").is_err());
        assert!(password_valida("Ñandú123").is_ok());
    }

    #[test]
    fn nombres() {
        assert!(nombre_valido("Ana").is_ok());
        assert!(nombre_valido("   ").is_err());
        assert!(nombre_valido(&"a".repeat(101)).is_err());
    }

    #[test]
    fn urls_solo_http() {
        assert!(url_http("x", "https://instagram.com/negocio").is_ok());
        assert!(url_http("x", "HTTP://ejemplo.com").is_ok());
        assert!(url_http("x", "").is_ok());
        for malo in [
            "javascript:alert(1)",
            "data:text/html,<script>",
            "ftp://x.com",
            "instagram.com/x",
            "https://a b.com",
            "//x.com",
        ] {
            assert!(url_http("x", malo).is_err(), "{malo}");
        }
        assert!(url_http_opt("x", &None).is_ok());
        assert!(url_http_opt("x", &Some("javascript:1".into())).is_err());
    }

    #[test]
    fn largos_maximos_en_caracteres() {
        assert!(texto_hasta("x", &"ñ".repeat(80), 80).is_ok());
        assert!(texto_hasta("x", &"a".repeat(81), 80).is_err());
        assert!(texto_hasta_opt("x", &None, 1).is_ok());
    }
}
