//! `fixture DIR`: writes a synthetic session fixture (`session::fixture`) for
//! client tests in other languages.
fn main() -> Result<(), String> {
    let dir = std::env::args().nth(1).ok_or("usage: fixture DIR")?;
    session::fixture::write(std::path::Path::new(&dir))
}
