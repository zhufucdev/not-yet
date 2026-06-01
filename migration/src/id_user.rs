use sea_orm_migration::prelude::*;

/// Identifiers for the `user` table.
/// Note: SeaORM's `rename_all = "camelCase"` affects column naming at the ORM
/// layer; the underlying DB columns use the field names as-is unless explicitly
/// renamed. Column names here match what the derive macro emits for a camelCase
/// table — adjust if your DB driver applies the rename to column identifiers.
#[derive(DeriveIden, Clone)]
pub enum User {
    Table,
    Id,
    AccessLevel,
}
