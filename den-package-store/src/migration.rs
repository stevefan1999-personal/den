use sea_orm_migration::{
    prelude::*,
    schema::{
        big_integer, big_integer_null, blob as blob_column, pk_auto, text, text_null, text_uniq,
    },
};

use crate::entity::{
    blob, dependency, package, package_export, package_file, package_version, registry,
};

pub struct Migrator;

const MIGRATION_TABLE: &str = "den_package_store_migrations";

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> { vec![Box::new(Migration)] }

    fn migration_table_name() -> DynIden { MigrationLog::Table.into_iden() }
}

#[derive(Iden)]
enum MigrationLog {
    #[iden = "den_package_store_migrations"]
    Table,
    Version,
    AppliedAt,
}

pub async fn install_tracking_table(manager: &SchemaManager<'_>) -> std::result::Result<(), DbErr> {
    if manager.has_table(MIGRATION_TABLE).await? {
        return Ok(());
    }
    manager.create_table(tracking_table()).await
}

pub fn expected_tables() -> Vec<(&'static str, TableCreateStatement)> {
    vec![
        (MIGRATION_TABLE, tracking_table()),
        ("blob", blob_table()),
        ("registry", registry_table()),
        ("package", package_table()),
        ("package_version", package_version_table()),
        ("dependency", dependency_table()),
        ("export", export_table()),
        ("package_file", package_file_table()),
    ]
}

fn tracking_table() -> TableCreateStatement {
    Table::create()
        .table(MigrationLog::Table)
        .if_not_exists()
        .col(text(MigrationLog::Version).primary_key())
        .col(big_integer(MigrationLog::AppliedAt))
        .extra("STRICT")
        .to_owned()
}

fn blob_table() -> TableCreateStatement {
    Table::create()
        .table(blob::Entity)
        .col(blob_column(blob::Column::Digest).primary_key())
        .col(blob_column(blob::Column::Bytes))
        .check(Expr::cust("length(\"digest\") = 32"))
        .extra("STRICT")
        .to_owned()
}

fn registry_table() -> TableCreateStatement {
    Table::create()
        .table(registry::Entity)
        .col(pk_auto(registry::Column::Id))
        .col(text(registry::Column::Kind))
        .col(text_uniq(registry::Column::BaseUrl))
        .extra("STRICT")
        .to_owned()
}

fn package_table() -> TableCreateStatement {
    Table::create()
        .table(package::Entity)
        .col(pk_auto(package::Column::Id))
        .col(big_integer(package::Column::RegistryId))
        .col(text(package::Column::Name))
        .foreign_key(
            ForeignKey::create()
                .from(package::Entity, package::Column::RegistryId)
                .to(registry::Entity, registry::Column::Id),
        )
        .index(
            Index::create()
                .name("uq-package-registry-name")
                .unique()
                .col(package::Column::RegistryId)
                .col(package::Column::Name),
        )
        .extra("STRICT")
        .to_owned()
}

fn package_version_table() -> TableCreateStatement {
    Table::create()
        .table(package_version::Entity)
        .col(pk_auto(package_version::Column::Id))
        .col(big_integer(package_version::Column::PackageId))
        .col(text(package_version::Column::Version))
        .col(big_integer_null(package_version::Column::PublishedAt))
        .col(text_null(package_version::Column::YankedReason))
        .col(blob_column(package_version::Column::ManifestDigest))
        .foreign_key(
            ForeignKey::create()
                .from(package_version::Entity, package_version::Column::PackageId)
                .to(package::Entity, package::Column::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(
                    package_version::Entity,
                    package_version::Column::ManifestDigest,
                )
                .to(blob::Entity, blob::Column::Digest),
        )
        .index(
            Index::create()
                .name("uq-package-version")
                .unique()
                .col(package_version::Column::PackageId)
                .col(package_version::Column::Version),
        )
        .extra("STRICT")
        .to_owned()
}

fn dependency_table() -> TableCreateStatement {
    Table::create()
        .table(dependency::Entity)
        .col(big_integer(dependency::Column::VersionId))
        .col(big_integer(dependency::Column::Ordinal))
        .col(text(dependency::Column::Kind))
        .col(big_integer_null(dependency::Column::TargetRegistryId))
        .col(text(dependency::Column::PackageName))
        .col(text(dependency::Column::Requirement))
        .col(text_null(dependency::Column::Alias))
        .primary_key(
            Index::create()
                .col(dependency::Column::VersionId)
                .col(dependency::Column::Ordinal),
        )
        .foreign_key(
            ForeignKey::create()
                .from(dependency::Entity, dependency::Column::VersionId)
                .to(package_version::Entity, package_version::Column::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(dependency::Entity, dependency::Column::TargetRegistryId)
                .to(registry::Entity, registry::Column::Id),
        )
        .extra("STRICT")
        .to_owned()
}

fn export_table() -> TableCreateStatement {
    Table::create()
        .table(package_export::Entity)
        .col(big_integer(package_export::Column::VersionId))
        .col(text(package_export::Column::Name))
        .col(text(package_export::Column::TargetPath))
        .primary_key(
            Index::create()
                .col(package_export::Column::VersionId)
                .col(package_export::Column::Name),
        )
        .foreign_key(
            ForeignKey::create()
                .from(package_export::Entity, package_export::Column::VersionId)
                .to(package_version::Entity, package_version::Column::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .extra("STRICT")
        .to_owned()
}

fn package_file_table() -> TableCreateStatement {
    Table::create()
        .table(package_file::Entity)
        .col(big_integer(package_file::Column::VersionId))
        .col(text(package_file::Column::Path))
        .col(blob_column(package_file::Column::BlobDigest))
        .col(text_null(package_file::Column::MediaType))
        .col(big_integer(package_file::Column::Mode))
        .primary_key(
            Index::create()
                .col(package_file::Column::VersionId)
                .col(package_file::Column::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(package_file::Entity, package_file::Column::VersionId)
                .to(package_version::Entity, package_version::Column::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(package_file::Entity, package_file::Column::BlobDigest)
                .to(blob::Entity, blob::Column::Digest),
        )
        .extra("STRICT")
        .to_owned()
}

#[derive(DeriveMigrationName)]
struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> { Some(true) }

    async fn up(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        for (_name, table) in expected_tables().into_iter().skip(1) {
            manager.create_table(table).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> std::result::Result<(), DbErr> {
        for table in [
            package_file::Entity.into_iden(),
            package_export::Entity.into_iden(),
            dependency::Entity.into_iden(),
            package_version::Entity.into_iden(),
            package::Entity.into_iden(),
            registry::Entity.into_iden(),
            blob::Entity.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}
