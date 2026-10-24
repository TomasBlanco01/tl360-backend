use crate::errors::{AppError, AppResult};

/// bcrypt solo usa los primeros 72 bytes de la contraseña; se limita para no aceptar claves que
/// en realidad se truncan en silencio ni pagar el costo de hashear entradas gigantes.
pub fn password_valida(p: &str) -> AppResult<()> {
    if p.chars().count() < 6 {
        return Err(AppError::BadRequest(
            "La contraseña debe tener al menos 6 caracteres".into(),
        ));
    }
    if p.len() > 72 {
        return Err(AppError::BadRequest(
            "La contraseña no puede superar los 72 caracteres".into(),
        ));
    }
    Ok(())
}

pub fn email_valido(e: &str) -> AppResult<()> {
    let e = e.trim();
    let mut partes = e.split('@');
    let (local, dominio) = (partes.next().unwrap_or(""), partes.next().unwrap_or(""));
    let ok = e.len() <= 254
        && partes.next().is_none()
        && !local.is_empty()
        && dominio.contains('.')
        && !dominio.starts_with('.')
        && !dominio.ends_with('.')
        && !e.chars().any(char::is_whitespace);
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "El correo electrónico no es válido".into(),
        ))
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
        ] {
            assert!(email_valido(e).is_err(), "{e}");
        }
        assert!(email_valido(&format!("{}@x.com", "a".repeat(260))).is_err());
    }

    #[test]
    fn contrasenas() {
        assert!(password_valida("12345").is_err());
        assert!(password_valida("123456").is_ok());
        assert!(password_valida(&"x".repeat(72)).is_ok());
        assert!(password_valida(&"x".repeat(73)).is_err());
        // se cuentan caracteres para el mínimo, no bytes
        assert!(password_valida("ñññññ").is_err());
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
}
