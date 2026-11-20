#![cfg(feature = "live-docker")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use muninn_aether_adapters::kv::redis::{RedisConfig, RedisRecordStore};
use muninn_aether_adapters::kv::sqlite::{SqliteConfig, SqliteRecordStore};
use muninn_aether_adapters::storage::chroma::{ChromaConfig, ChromaVectorStore};
use muninn_aether_adapters::storage::neo4j::{Neo4jConfig, Neo4jGraphStore};
use muninn_aether_adapters::storage::pgvector::{PgVectorConfig, PgVectorStore};
use muninn_aether_adapters::storage::qdrant::{QdrantConfig, QdrantVectorStore};
use muninn_aether_core::{
    AetherError, ConsistencyMode, DistanceMetric, Entity, EntityId, MemoryInput, MemoryScope,
    MemoryUpdate, ProjectId, RecallMode, RecallQuery, ReceiptState, Relation, SecretString,
    SessionId, UserId, VectorStore, WriteDurability,
};
use muninn_aether_sdk::MemoryManager;
use uuid::Uuid;

struct TempSqliteDb {
    path: PathBuf,
}

impl TempSqliteDb {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("aether-live-{label}-{}.db", Uuid::new_v4()));
        Self { path }
    }

    fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempSqliteDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{}-wal", self.path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

fn neo4j_store() -> Neo4jGraphStore {
    let base_url =
        std::env::var("NEO4J_URL").unwrap_or_else(|_| "http://127.0.0.1:17474".to_string());
    let database = std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "neo4j".to_string());
    let username = std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
    let password = std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "testpassword".to_string());
    Neo4jGraphStore::new(Neo4jConfig {
        base_url,
        database,
        username,
        password: SecretString::new(password),
    })
    .unwrap()
}

async fn verify_full_manager_lifecycle(
    mgr_a: &MemoryManager,
    mgr_rebuilt: &MemoryManager,
    ns: &ProjectId,
) {
    let alice = Entity {
        id: EntityId::new("person:alice").unwrap(),
        name: "Alice".into(),
        entity_type: "Person".into(),
    };
    let bob = Entity {
        id: EntityId::new("person:bob").unwrap(),
        name: "Bob".into(),
        entity_type: "Person".into(),
    };

    // 1. Buffered write survives across rebuilt MemoryManager and is readable immediately.
    let mut m1 = MemoryInput::new(ns.clone(), "Alice works with Bob");
    m1.embedding = Some(vec![1.0, 0.0, 0.0]);
    m1.durability = WriteDurability::Buffered;
    m1.idempotency_key = Some(format!("idem-m1-{}", Uuid::new_v4()));
    m1.entities = vec![alice.clone(), bob.clone()];
    m1.relations = vec![Relation {
        source: alice.id.clone(),
        target: bob.id.clone(),
        relation_type: "WORKS_WITH".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let r1 = mgr_a.remember(m1.clone()).await.unwrap();
    assert_eq!(r1.state, ReceiptState::Buffered);

    // Duplicate idempotency key returns original memory id.
    let r1_dup = mgr_a.remember(m1).await.unwrap();
    assert_eq!(r1_dup.memory_id, r1.memory_id);

    // Eventual recall misses buffered write before flush; ReadYourWrites finds it.
    let mut q_ev = RecallQuery::new(ns.clone(), "Alice");
    q_ev.query_vector = Some(vec![1.0, 0.0, 0.0]);
    q_ev.mode = RecallMode::Vector;
    q_ev.consistency = ConsistencyMode::Eventual;
    assert!(mgr_a.recall(q_ev.clone()).await.unwrap().hits.is_empty());

    let mut q_ryw = q_ev.clone();
    q_ryw.consistency = ConsistencyMode::ReadYourWrites;
    assert_eq!(
        mgr_a.recall(q_ryw).await.unwrap().hits[0].memory_id,
        r1.memory_id
    );

    // Rebuilt manager flushes m1 into derived vector/graph indexes.
    mgr_rebuilt.flush().await.unwrap();
    assert_eq!(
        mgr_rebuilt.recall(q_ev).await.unwrap().hits[0].memory_id,
        r1.memory_id
    );

    // 2. Graph-only memory surfaced via Hybrid recall without caller seeds.
    let mut m2 = MemoryInput::new(ns.clone(), "Bob runs infra cluster");
    m2.durability = WriteDurability::WaitFlush;
    m2.entities = vec![bob.clone()];
    m2.relations = vec![Relation {
        source: bob.id.clone(),
        target: EntityId::new("team:infra").unwrap(),
        relation_type: "LEADS".into(),
        valid_at: Utc::now(),
        invalid_at: None,
    }];
    let r2 = mgr_rebuilt.remember(m2).await.unwrap();
    assert_eq!(r2.state, ReceiptState::Persisted);

    let mut q_hybrid = RecallQuery::new(ns.clone(), "Alice");
    q_hybrid.query_vector = Some(vec![1.0, 0.0, 0.0]);
    q_hybrid.mode = RecallMode::Hybrid;
    q_hybrid.top_k = 5;
    let hybrid_res = mgr_rebuilt.recall(q_hybrid).await.unwrap();
    assert!(hybrid_res.hits.iter().any(|h| h.memory_id == r2.memory_id));

    // 3. User scope isolation.
    let u1 = UserId::new("u1").unwrap();
    let u2 = UserId::new("u2").unwrap();
    let mut u1_in = MemoryInput::new(ns.clone(), "u1 secret");
    u1_in.scope = MemoryScope::User(u1.clone());
    u1_in.embedding = Some(vec![0.0, 1.0, 0.0]);
    u1_in.durability = WriteDurability::WaitFlush;
    let ru1 = mgr_rebuilt.remember(u1_in).await.unwrap();

    let mut u2_in = MemoryInput::new(ns.clone(), "u2 secret");
    u2_in.scope = MemoryScope::User(u2);
    u2_in.embedding = Some(vec![0.0, 1.0, 0.0]);
    u2_in.durability = WriteDurability::WaitFlush;
    mgr_rebuilt.remember(u2_in).await.unwrap();

    let mut q_u1 = RecallQuery::new(ns.clone(), "secret");
    q_u1.query_vector = Some(vec![0.0, 1.0, 0.0]);
    q_u1.mode = RecallMode::Vector;
    q_u1.scopes = vec![MemoryScope::User(u1)];
    let u1_hits = mgr_rebuilt.recall(q_u1).await.unwrap().hits;
    assert_eq!(u1_hits.len(), 1);
    assert_eq!(u1_hits[0].memory_id, ru1.memory_id);

    // 4. Session memory works live and expires after TTL.
    let sid = SessionId::new("live-sess").unwrap();
    let mut sess_in = MemoryInput::new(ns.clone(), "short turn");
    sess_in.scope = MemoryScope::Session(sid.clone());
    sess_in.embedding = Some(vec![0.0, 0.0, 1.0]);
    let r_sess = mgr_rebuilt.remember(sess_in).await.unwrap();
    mgr_rebuilt.flush().await.unwrap();

    let mut q_sess = RecallQuery::new(ns.clone(), "turn");
    q_sess.query_vector = Some(vec![0.0, 0.0, 1.0]);
    q_sess.mode = RecallMode::Vector;
    q_sess.scopes = vec![MemoryScope::Session(sid)];
    assert_eq!(
        mgr_rebuilt.recall(q_sess.clone()).await.unwrap().hits[0].memory_id,
        r_sess.memory_id
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(mgr_rebuilt.recall(q_sess).await.unwrap().hits.is_empty());

    // 5. Update content requires new embedding; CAS conflict & forget work end-to-end.
    let bad_upd = mgr_rebuilt
        .update(
            ns,
            r1.memory_id,
            MemoryUpdate {
                content: Some("new text no vec".into()),
                embedding: None,
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(1),
            },
        )
        .await;
    assert!(matches!(bad_upd, Err(AetherError::InvalidInput(_))));

    let upd = mgr_rebuilt
        .update(
            ns,
            r1.memory_id,
            MemoryUpdate {
                content: Some("Alice works closely with Bob".into()),
                embedding: Some(vec![1.0, 0.0, 0.0]),
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(upd.version, 2);

    let stale = mgr_rebuilt
        .update(
            ns,
            r1.memory_id,
            MemoryUpdate {
                content: Some("stale".into()),
                embedding: Some(vec![1.0, 0.0, 0.0]),
                metadata: None,
                entities: None,
                relations: None,
                expected_version: Some(1),
            },
        )
        .await;
    assert!(matches!(stale, Err(AetherError::Conflict { .. })));

    mgr_rebuilt.forget(ns, r1.memory_id).await.unwrap();
    let mut q_after_forget = RecallQuery::new(ns.clone(), "Alice");
    q_after_forget.query_vector = Some(vec![1.0, 0.0, 0.0]);
    q_after_forget.mode = RecallMode::Vector;
    assert!(mgr_rebuilt
        .recall(q_after_forget)
        .await
        .unwrap()
        .hits
        .iter()
        .all(|h| h.memory_id != r1.memory_id));
}

#[tokio::test]
#[ignore = "requires live Redis/Valkey, pgvector, and Neo4j containers"]
async fn live_e2e_redis_pgvector_neo4j() {
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:56379".to_string());
    let pg_url = std::env::var("PGVECTOR_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://aether:aether@127.0.0.1:55433/aether".to_string());
    let table = format!("live_e2e_{}", Uuid::new_v4().simple());
    let prefix = format!("live-e2e-{}", Uuid::new_v4().simple());
    let ns = ProjectId::new(format!("e2e-rpg-{}", Uuid::new_v4())).unwrap();

    let rec_a = Arc::new(
        RedisRecordStore::connect(RedisConfig::new(redis_url.clone(), prefix.clone()))
            .await
            .unwrap(),
    );
    let vec_a = Arc::new(
        PgVectorStore::connect(PgVectorConfig {
            database_url: SecretString::new(pg_url.clone()),
            table_name: table.clone(),
            max_connections: 4,
        })
        .await
        .unwrap(),
    );
    vec_a
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    let graph_a = Arc::new(neo4j_store());

    let mgr_a = MemoryManager::builder(rec_a)
        .vector(vec_a)
        .graph(graph_a)
        .with_session_ttl(Some(Duration::from_millis(50)))
        .build();

    let rec_b = Arc::new(
        RedisRecordStore::connect(RedisConfig::new(redis_url, prefix))
            .await
            .unwrap(),
    );
    let vec_b = Arc::new(
        PgVectorStore::connect(PgVectorConfig {
            database_url: SecretString::new(pg_url),
            table_name: table,
            max_connections: 4,
        })
        .await
        .unwrap(),
    );
    let graph_b = Arc::new(neo4j_store());

    let mgr_b = MemoryManager::builder(rec_b)
        .vector(vec_b)
        .graph(graph_b)
        .with_session_ttl(Some(Duration::from_millis(50)))
        .build();

    verify_full_manager_lifecycle(&mgr_a, &mgr_b, &ns).await;
    mgr_b.shutdown().await.unwrap();
    mgr_a.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires live Qdrant and Neo4j containers"]
async fn live_e2e_sqlite_qdrant_neo4j() {
    let db = TempSqliteDb::new("qdrant-e2e");
    let qdrant_url =
        std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:16333".to_string());
    let prefix = format!("e2e-q-{}", Uuid::new_v4().simple());
    let ns = ProjectId::new(format!("e2e-sqn-{}", Uuid::new_v4())).unwrap();

    let rec_a = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .unwrap(),
    );
    let vec_a = Arc::new(
        QdrantVectorStore::new(QdrantConfig {
            base_url: qdrant_url.clone(),
            api_key: None,
            collection_prefix: prefix.clone(),
        })
        .unwrap(),
    );
    vec_a
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr_a = MemoryManager::builder(rec_a)
        .vector(vec_a)
        .graph(Arc::new(neo4j_store()))
        .with_session_ttl(Some(Duration::from_millis(50)))
        .build();

    let rec_b = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .unwrap(),
    );
    let vec_b = Arc::new(
        QdrantVectorStore::new(QdrantConfig {
            base_url: qdrant_url,
            api_key: None,
            collection_prefix: prefix,
        })
        .unwrap(),
    );
    let mgr_b = MemoryManager::builder(rec_b)
        .vector(vec_b)
        .graph(Arc::new(neo4j_store()))
        .with_session_ttl(Some(Duration::from_millis(50)))
        .build();

    verify_full_manager_lifecycle(&mgr_a, &mgr_b, &ns).await;
    mgr_b.shutdown().await.unwrap();
    mgr_a.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires live Chroma and Neo4j containers"]
async fn live_e2e_sqlite_chroma_neo4j() {
    let db = TempSqliteDb::new("chroma-e2e");
    let chroma_url =
        std::env::var("CHROMA_URL").unwrap_or_else(|_| "http://127.0.0.1:18000".to_string());
    let prefix = format!("e2e_c_{}", Uuid::new_v4().simple());
    let ns = ProjectId::new(format!("e2e-scn-{}", Uuid::new_v4())).unwrap();

    let rec_a = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .unwrap(),
    );
    let vec_a = Arc::new(
        ChromaVectorStore::new(ChromaConfig {
            base_url: chroma_url.clone(),
            tenant: "default_tenant".into(),
            database: "default_database".into(),
            auth_token: None,
            collection_prefix: prefix.clone(),
        })
        .unwrap(),
    );
    vec_a
        .initialize(&ns, 3, DistanceMetric::Cosine)
        .await
        .unwrap();
    let mgr_a = MemoryManager::builder(rec_a)
        .vector(vec_a)
        .graph(Arc::new(neo4j_store()))
        .with_session_ttl(Some(Duration::from_millis(50)))
        .build();

    let rec_b = Arc::new(
        SqliteRecordStore::open(SqliteConfig::from_path(db.path_str()))
            .await
            .unwrap(),
    );
    let vec_b = Arc::new(
        ChromaVectorStore::new(ChromaConfig {
            base_url: chroma_url,
            tenant: "default_tenant".into(),
            database: "default_database".into(),
            auth_token: None,
            collection_prefix: prefix,
        })
        .unwrap(),
    );
    let mgr_b = MemoryManager::builder(rec_b)
        .vector(vec_b)
        .graph(Arc::new(neo4j_store()))
        .with_session_ttl(Some(Duration::from_millis(50)))
        .build();

    verify_full_manager_lifecycle(&mgr_a, &mgr_b, &ns).await;
    mgr_b.shutdown().await.unwrap();
    mgr_a.shutdown().await.unwrap();
}
