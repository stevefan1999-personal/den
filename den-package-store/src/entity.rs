pub mod blob {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "blob")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub digest: Vec<u8>,
        pub bytes:  Vec<u8>,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod registry {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "registry")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id:       i64,
        pub kind:     String,
        #[sea_orm(unique)]
        pub base_url: String,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod package {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "package")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id:          i64,
        pub registry_id: i64,
        pub name:        String,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod package_version {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "package_version")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id:              i64,
        pub package_id:      i64,
        pub version:         String,
        pub published_at:    Option<i64>,
        pub yanked_reason:   Option<String>,
        pub manifest_digest: Vec<u8>,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod dependency {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "dependency")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub version_id:         i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub ordinal:            i64,
        pub kind:               String,
        pub target_registry_id: Option<i64>,
        pub package_name:       String,
        pub requirement:        String,
        pub alias:              Option<String>,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod package_export {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "export")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub version_id:  i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub name:        String,
        pub target_path: String,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod package_file {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, Eq, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "package_file")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub version_id:  i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub path:        String,
        pub blob_digest: Vec<u8>,
        pub media_type:  Option<String>,
        pub mode:        i64,
    }

    #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
