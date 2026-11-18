#![cfg(feature = "pgvector")]

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;
use std::io;

use aether_adapters::storage::pgvector::{
    build_search_query, map_sqlx_error, validate_sql_identifier, PgVectorConfig, PgVectorStore,
};
use aether_core::{
    AetherError, DistanceMetric, FilterExpr, FilterValue, OutboundConnector, ProjectId,
    SecretString, VectorStore,
};
use sqlx::error::{DatabaseError, ErrorKind};
use sqlx::{Arguments, Execute};

#[derive(Debug)]
struct FakePgDatabaseError {
    code: Option<String>,
    message: String,
}

impl FakePgDatabaseError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: Some(code.to_string()),
            message: message.to_string(),
        }
    }
}

impl fmt::Display for FakePgDatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for FakePgDatabaseError {}

impl DatabaseError for FakePgDatabaseError {
    fn message(&self) -> &str {
        &self.message
    }

    fn code(&self) -> Option<Cow<'_, str>> {
        self.code.as_deref().map(Cow::Borrowed)
    }

    fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> ErrorKind {
        match self.code.as_deref() {
            Some("23505") => ErrorKind::UniqueViolation,
            Some("23503") => ErrorKind::ForeignKeyViolation,
            Some("23502") => ErrorKind::NotNullViolation,
            Some("23514") => ErrorKind::CheckViolation,
            _ => ErrorKind::Other,
        }
    }
}

fn sample_config(table_name: &str) -> PgVectorConfig {
    PgVectorConfig {
        database_url: SecretString::from(
            "postgres://alice:super_secret_pw@db.internal:5432/aether?sslmode=require",
        ),
        table_name: table_name.to_string(),
        max_connections: 8,
    }
}

#[tokio::test]
async fn table_name_validation_accepts_valid_identifiers_and_rejects_injections() {
    for valid in ["aether_vectors", "_vectors_v2", "Vectors01", "a"] {
        assert!(
            validate_sql_identifier(valid).is_ok(),
            "expected `{valid}` to be accepted"
        );
        assert!(
            sample_config(valid).validate().is_ok(),
            "expected config with `{valid}` to validate"
        );
        assert!(
            PgVectorStore::connect_lazy(sample_config(valid)).is_ok(),
            "expected connect_lazy with `{valid}` to succeed"
        );
    }

    for invalid in [
        "",
        "123vectors",
        "vectors-v1",
        "public.vectors",
        "vectors; DROP TABLE users;--",
        "vectors name",
        "vectors'quote",
        "vectors\"quote",
        "vectors()",
    ] {
        let err = validate_sql_identifier(invalid).expect_err("invalid identifier must fail");
        assert!(
            matches!(err, AetherError::InvalidInput(_)),
            "expected InvalidInput for `{invalid}`, got {err:?}"
        );

        let store_err = PgVectorStore::connect_lazy(sample_config(invalid))
            .expect_err("connect_lazy must reject invalid table_name");
        assert!(matches!(store_err, AetherError::InvalidInput(_)));
    }

    let empty_url_cfg = PgVectorConfig {
        database_url: SecretString::from("   "),
        table_name: "aether_vectors".to_string(),
        max_connections: 4,
    };
    assert!(matches!(
        empty_url_cfg.validate(),
        Err(AetherError::InvalidInput(_))
    ));

    let zero_conn_cfg = PgVectorConfig {
        database_url: SecretString::from("postgres://localhost/aether"),
        table_name: "aether_vectors".to_string(),
        max_connections: 0,
    };
    assert!(matches!(
        zero_conn_cfg.validate(),
        Err(AetherError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn config_and_store_debug_redact_database_url_userinfo() {
    let cfg = sample_config("aether_vectors");
    let cfg_debug = format!("{cfg:?}");

    assert!(!cfg_debug.contains("super_secret_pw"));
    assert!(!cfg_debug.contains("alice"));
    assert!(cfg_debug.contains("postgres://[REDACTED]@db.internal:5432/aether?sslmode=require"));
    assert!(cfg_debug.contains("aether_vectors"));
    assert!(cfg_debug.contains("8"));

    let store = PgVectorStore::connect_lazy(cfg).expect("valid lazy store");
    let store_debug = format!("{store:?}");
    assert!(!store_debug.contains("super_secret_pw"));
    assert!(!store_debug.contains("alice"));
    assert!(store_debug.contains("aether_vectors"));
}

#[tokio::test]
async fn outbound_connector_reports_pgvector_name_and_capabilities() {
    let store = PgVectorStore::connect_lazy(sample_config("aether_vectors")).unwrap();
    assert_eq!(store.name(), "pgvector");
    let caps = store.capabilities();
    assert!(caps.supports_filter_expr);
    assert!(!caps.supports_temporal);
    assert!(caps.max_batch_size > 0);
}

#[test]
fn filter_expr_compiles_parameterized_sql_and_binds_values() {
    let ns = ProjectId::new("proj-secret-ns'; DROP TABLE aether_vectors;--").unwrap();
    let injection_value = "O'Brien'; DROP TABLE aether_vectors;--";
    let filter = FilterExpr::And(vec![
        FilterExpr::Eq {
            field: "author".into(),
            value: FilterValue::String(injection_value.into()),
        },
        FilterExpr::In {
            field: "tier".into(),
            values: vec![
                FilterValue::String("semantic".into()),
                FilterValue::Int(42),
                FilterValue::Bool(true),
            ],
        },
        FilterExpr::Range {
            field: "score".into(),
            gte: Some(0.5),
            lte: Some(0.95),
        },
        FilterExpr::Or(vec![
            FilterExpr::Eq {
                field: "weight".into(),
                value: FilterValue::Float(1.5),
            },
            FilterExpr::Range {
                field: "age".into(),
                gte: None,
                lte: Some(10.0),
            },
        ]),
    ]);

    let mut qb = build_search_query(
        "aether_vectors",
        &ns,
        &[1.0, 0.0, 0.0],
        5,
        DistanceMetric::Cosine,
        Some(&filter),
    )
    .expect("valid filter compiles");

    let mut query = qb.build();
    let args = query
        .take_arguments()
        .expect("take_arguments ok")
        .expect("arguments present");
    let sql_str = query.sql();
    let sql = sql_str.as_str();

    assert!(sql.starts_with(
        "SELECT id, (1.0 - (embedding <=> $1))::double precision AS score, metadata FROM aether_vectors WHERE ns = $2 AND "
    ));
    assert!(sql.contains("ORDER BY embedding <=> $20 ASC LIMIT $21"));
    assert_eq!(args.len(), 21);

    assert!(!sql.contains("O'Brien"));
    assert!(!sql.contains("DROP TABLE"));
    assert!(!sql.contains("semantic"));
    assert!(!sql.contains("proj-secret-ns"));
    assert!(!sql.contains("0.95"));
    assert!(!sql.contains("42"));
}

#[test]
fn filter_expr_handles_empty_combinators_and_open_ranges() {
    let ns = ProjectId::new("proj-edge").unwrap();

    let empty_and = FilterExpr::And(vec![]);
    let mut qb = build_search_query(
        "aether_vectors",
        &ns,
        &[0.1, 0.9],
        3,
        DistanceMetric::Dot,
        Some(&empty_and),
    )
    .unwrap();
    let mut query = qb.build();
    let args = query.take_arguments().unwrap().unwrap();
    let sql_str = query.sql();
    let sql = sql_str.as_str();
    assert!(sql.contains("(-(embedding <#> $1))::double precision AS score"));
    assert!(sql.contains("WHERE ns = $2 AND TRUE ORDER BY embedding <#> $3 ASC LIMIT $4"));
    assert_eq!(args.len(), 4);

    let empty_or_and_in = FilterExpr::Or(vec![
        FilterExpr::Or(vec![]),
        FilterExpr::In {
            field: "tier".into(),
            values: vec![],
        },
        FilterExpr::Range {
            field: "score".into(),
            gte: None,
            lte: None,
        },
    ]);
    let mut qb2 = build_search_query(
        "aether_vectors",
        &ns,
        &[0.1, 0.9],
        3,
        DistanceMetric::Euclidean,
        Some(&empty_or_and_in),
    )
    .unwrap();
    let mut query2 = qb2.build();
    let args2 = query2.take_arguments().unwrap().unwrap();
    let sql2_str = query2.sql();
    let sql2 = sql2_str.as_str();
    assert!(sql2.contains("(-(embedding <-> $1))::double precision AS score"));
    assert!(sql2.contains("(FALSE OR FALSE OR jsonb_typeof(metadata -> $3::text) = 'number')"));
    assert!(sql2.contains("ORDER BY embedding <-> $4 ASC LIMIT $5"));
    assert_eq!(args2.len(), 5);
}

#[test]
fn filter_expr_rejects_unsafe_field_names_and_non_finite_floats() {
    let ns = ProjectId::new("proj-sec").unwrap();

    for bad_field in [
        "",
        "tier'; DROP TABLE aether_vectors;--",
        "metadata->>'tier'",
        "a.b",
        "1bad",
        "has space",
    ] {
        let eq = FilterExpr::Eq {
            field: bad_field.into(),
            value: FilterValue::String("x".into()),
        };
        let err = build_search_query(
            "aether_vectors",
            &ns,
            &[1.0, 0.0],
            5,
            DistanceMetric::Cosine,
            Some(&eq),
        )
        .err()
        .expect("unsafe field name must be rejected");
        assert!(matches!(err, AetherError::InvalidInput(_)));
    }

    let nan_eq = FilterExpr::Eq {
        field: "score".into(),
        value: FilterValue::Float(f64::NAN),
    };
    assert!(matches!(
        build_search_query(
            "aether_vectors",
            &ns,
            &[1.0, 0.0],
            5,
            DistanceMetric::Cosine,
            Some(&nan_eq),
        ),
        Err(AetherError::InvalidInput(_))
    ));

    let inf_range = FilterExpr::Range {
        field: "score".into(),
        gte: Some(f64::INFINITY),
        lte: None,
    };
    assert!(matches!(
        build_search_query(
            "aether_vectors",
            &ns,
            &[1.0, 0.0],
            5,
            DistanceMetric::Cosine,
            Some(&inf_range),
        ),
        Err(AetherError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn initialize_rejects_zero_dimensions_before_database_io() {
    let store = PgVectorStore::connect_lazy(sample_config("aether_vectors")).unwrap();
    let ns = ProjectId::new("proj-zero").unwrap();
    let err = store
        .initialize(&ns, 0, DistanceMetric::Cosine)
        .await
        .expect_err("zero dimensions must be rejected");
    assert!(matches!(err, AetherError::InvalidInput(_)));
}

#[test]
fn maps_sqlx_errors_to_transient_or_permanent() {
    let transient_cases = vec![
        sqlx::Error::PoolTimedOut,
        sqlx::Error::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "connection reset",
        )),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "40001",
            "could not serialize access",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "40P01",
            "deadlock detected",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "08006",
            "connection failure",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "53300",
            "too many clients already",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "57P03",
            "the database system is starting up",
        ))),
    ];

    for err in transient_cases {
        let mapped = map_sqlx_error(err);
        assert!(
            matches!(
                mapped,
                AetherError::Transient {
                    connector: "pgvector",
                    ..
                }
            ),
            "expected Transient, got {mapped:?}"
        );
        assert!(mapped.is_retryable());
    }

    let permanent_cases = vec![
        sqlx::Error::Configuration("invalid database URL".into()),
        sqlx::Error::PoolClosed,
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "42601",
            "syntax error at or near",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "23505",
            "duplicate key value violates unique constraint",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "28P01",
            "password authentication failed",
        ))),
        sqlx::Error::Database(Box::new(FakePgDatabaseError::new(
            "42P01",
            "relation does not exist",
        ))),
    ];

    for err in permanent_cases {
        let mapped = map_sqlx_error(err);
        assert!(
            matches!(
                mapped,
                AetherError::Permanent {
                    connector: "pgvector",
                    ..
                }
            ),
            "expected Permanent, got {mapped:?}"
        );
        assert!(!mapped.is_retryable());
    }
}

#[cfg(feature = "contract-suite")]
#[tokio::test]
#[ignore = "requires a running Postgres instance with pgvector (PGVECTOR_DATABASE_URL or default local docker)"]
async fn live_pgvector_contract_vector_store() {
    let url = std::env::var("PGVECTOR_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://aether:aether@127.0.0.1:55433/aether".to_string());
    let table = std::env::var("PGVECTOR_TEST_TABLE")
        .unwrap_or_else(|_| format!("aether_vec_{}", uuid::Uuid::new_v4().simple()));
    let store = PgVectorStore::connect(PgVectorConfig {
        database_url: SecretString::new(url.clone()),
        table_name: table.clone(),
        max_connections: 5,
    })
    .await
    .expect("connect to pgvector");

    assert_eq!(store.name(), "pgvector");
    assert_eq!(
        store.health_check().await.expect("health_check"),
        aether_core::ConnectorHealth::Healthy
    );
    aether_adapters::contract::vector_store(&store).await;

    let ns = ProjectId::new(format!("pg-desc-{}", uuid::Uuid::new_v4())).unwrap();
    store
        .initialize(&ns, 3, DistanceMetric::Euclidean)
        .await
        .unwrap();
    let fresh = PgVectorStore::connect(PgVectorConfig {
        database_url: SecretString::new(url),
        table_name: table,
        max_connections: 2,
    })
    .await
    .expect("reconnect fresh pgvector");
    assert_eq!(
        fresh.describe(&ns).await.unwrap(),
        Some(aether_core::IndexSpec {
            dimensions: 3,
            metric: DistanceMetric::Euclidean,
        })
    );
    assert!(matches!(
        fresh.initialize(&ns, 4, DistanceMetric::Euclidean).await,
        Err(AetherError::InvalidInput(_))
    ));
    assert!(matches!(
        fresh.search(&ns, &[1.0, 0.0], 5, None).await,
        Err(AetherError::InvalidInput(_))
    ));
    let missing = ProjectId::new("pg-missing").unwrap();
    assert_eq!(fresh.describe(&missing).await.unwrap(), None);

    fresh.close().await.expect("close fresh");
    store.close().await.expect("close");
}
