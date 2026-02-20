pub mod github;
pub mod gmail;
pub mod linear;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    Unauthorized(&'static str),
    BadRequest(&'static str),
}
