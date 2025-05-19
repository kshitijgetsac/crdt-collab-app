use actix::{Actor, AsyncContext, Handler, Running, StreamHandler};
use actix_cors::Cors;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use actix_web_actors::ws;
use actix_derive::Message as DeriveMessage;
use futures_util::StreamExt;
use mongodb::{
    bson::{doc, DateTime as BsonDateTime, Document},
    options::{ClientOptions, UpdateOptions},
    Client as MongoClient,
    Database,
};
use redis::{Client as RedisClient, AsyncCommands};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, HashSet}, sync::Arc};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{self, Duration};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;

// — Shared in-memory state
#[derive(Debug)]
struct DocState {
    content: String,
    timestamp: DateTime<Utc>,
}
type SharedDocs = Arc<AsyncMutex<HashMap<String, DocState>>>;

// — Track which rooms have had their demo thread spawned
static SPAWNED_ROOMS: Lazy<AsyncMutex<HashSet<String>>> =
    Lazy::new(|| AsyncMutex::new(HashSet::new()));

// — Update payload
#[derive(Debug, Deserialize, Serialize, Clone)]
struct UpdateRequest {
    room: String,
    content: String,
    timestamp: DateTime<Utc>,
}

// — Core LWW + Redis caching + pub/sub
async fn process_update(
    docs: web::Data<SharedDocs>,
    redis: web::Data<RedisClient>,
    req: UpdateRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut map = docs.lock().await;
    let entry = map.entry(req.room.clone()).or_insert(DocState {
        content: String::new(),
        timestamp: Utc::now(),
    });
    println!("in process update");
    if req.timestamp > entry.timestamp {
        entry.content = req.content.clone();
        entry.timestamp = req.timestamp;
        println!("inside condition");
        let mut conn = redis.get_async_connection().await?;
        let key = format!("doc:{}", &req.room);
        println!("key is {:?}",key);
        let _: () = conn.set_ex(&key, &entry.content, 300).await?;
        let payload = serde_json::to_string(&req)?;
        let _: () = conn.publish(&req.room, payload).await?;
    }
    Ok(())
}

#[post("/api/update")]
async fn update_doc(
    docs: web::Data<SharedDocs>,
    redis: web::Data<RedisClient>,
    req: web::Json<UpdateRequest>,
) -> impl Responder {
    if let Err(err) = process_update(docs.clone(), redis.clone(), req.into_inner()).await {
        eprintln!("Update error: {}", err);
    }
    HttpResponse::Ok().finish()
}

// — GET handler: Redis → in-memory → MongoDB
#[get("/api/doc/{room}")]
async fn get_doc(
    docs: web::Data<SharedDocs>,
    redis: web::Data<RedisClient>,
    db: web::Data<Database>,
    path: web::Path<String>,
) -> impl Responder {
    let room = path.into_inner();

    // 1) Redis cache
    if let Ok(mut conn) = redis.get_async_connection().await {
        let key = format!("doc:{}", &room);
        if let Ok(content) = conn.get::<_, String>(&key).await {
            return HttpResponse::Ok().body(content);
        }
    }

    // 2) In-memory
    {
        let map = docs.lock().await;
        if let Some(state) = map.get(&room) {
            return HttpResponse::Ok().body(state.content.clone());
        }
    }

    // 3) MongoDB fallback
    {
        let coll = db.collection::<Document>("docs");
        if let Ok(Some(doc_json)) = coll.find_one(doc! { "room": &room }, None).await {
            if let Some(content) = doc_json
                .get("content")
                .and_then(|v| v.as_str())
            {
                return HttpResponse::Ok().body(content.to_owned());
            }
        }
    }

    // 4) Not found
    HttpResponse::NotFound().finish()
}

// — WebSocket actor boilerplate
#[derive(DeriveMessage)]
#[rtype(result = "()")]
struct WsMessage(String);

struct MyWsSession {
    room: String,
    docs: SharedDocs,
    redis: RedisClient,
}

impl MyWsSession {
    fn new(room: String, docs: SharedDocs, redis: RedisClient) -> Self {
        Self { room, docs, redis }
    }
}

impl Actor for MyWsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let client = self.redis.clone();
        let room = self.room.clone();
        let addr = ctx.address();
        actix::spawn(async move {
            let mut pubsub = client.get_async_connection().await.unwrap().into_pubsub();
            pubsub.subscribe(&room).await.unwrap();
            let mut stream = pubsub.on_message();
            while let Some(msg) = stream.next().await {
                let payload: String = msg.get_payload().unwrap();
                addr.do_send(WsMessage(payload));
            }
        });
    }

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        Running::Stop
    }
}

impl Handler<WsMessage> for MyWsSession {
    type Result = ();
    fn handle(&mut self, msg: WsMessage, ctx: &mut Self::Context) {
        ctx.text(msg.0);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for MyWsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, _: &mut Self::Context) {
        if let Ok(ws::Message::Text(text)) = msg {
            if let Ok(update) = serde_json::from_str::<UpdateRequest>(&text) {
                let docs  = web::Data::new(self.docs.clone());
                let redis = web::Data::new(self.redis.clone());
                actix::spawn(async move {
                    let _ = process_update(docs, redis, update).await;
                });
            }
        }
    }
}

async fn ws_index(
    req: actix_web::HttpRequest,
    stream: web::Payload,
    path: web::Path<String>,
    docs: web::Data<SharedDocs>,
    redis: web::Data<RedisClient>,
) -> impl Responder {
    let room = path.into_inner();

    // Spawn demo thread only once per room
    {
        let mut spawned = SPAWNED_ROOMS.lock().await;
        if spawned.insert(room.clone()) {
            let thread_room = room.clone();
            std::thread::spawn(move || {
                println!("[Demo Thread] Room initialized: {}", thread_room);
            });
        }
    }

    ws::start(
        MyWsSession::new(room, docs.get_ref().clone(), redis.get_ref().clone()),
        &req,
        stream,
    )
    .unwrap()
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // MongoDB setup
    let mut opts = ClientOptions::parse("mongodb+srv://kshitij:6092584904@cluster0.clhkh7t.mongodb.net")
        .await
        .unwrap();
    opts.app_name = Some("crdt-app".to_string());
    let client = MongoClient::with_options(opts).unwrap();
    let db: Database = client.database("crdt");
    println!("MongoDB connected to crdt");

    // Shared state + Redis client
    let docs: SharedDocs = Arc::new(AsyncMutex::new(HashMap::new()));
    let redis_client = RedisClient::open("redis://127.0.0.1:6379/").unwrap();

    // Background flush/upsert every 10 minutes
    {
        let db_clone   = db.clone();
        let docs_clone = docs.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(60));
            println!("inside loop");
            loop {
                interval.tick().await;
                let now = Utc::now();
                let map = docs_clone.lock().await;
                for (room, state) in map.iter() {
                    println!("map is there {:?}",map);
                    if now.signed_duration_since(state.timestamp).num_seconds() > 30 {
                        let coll   = db_clone.collection::<Document>("chatmessages");
                        let filter = doc! { "room": room };
                        let millis = state.timestamp.timestamp_millis();
                        let bson_ts = BsonDateTime::from_millis(millis);
                        let update_doc_bson = doc! {
                            "$set": {
                                "room":room,
                                "content": &state.content,
                                "timestamp": bson_ts
                            }
                        };
                        let opts = UpdateOptions::builder().upsert(true).build();
                        let _ = coll.update_one(filter, update_doc_bson, opts).await;
                    }
                }
            }
        });
    }

    // Start server with permissive CORS
    HttpServer::new(move || {
        App::new()
            .wrap(Cors::permissive())
            .app_data(web::Data::new(docs.clone()))
            .app_data(web::Data::new(redis_client.clone()))
            .app_data(web::Data::new(db.clone()))
            .service(update_doc)
            .service(get_doc)
            .route("/ws/{room}", web::get().to(ws_index))
            .route("/health", web::get().to(|| async { "ok" }))
    })
    .workers(8)
    .bind(("127.0.0.1", 1234))?
    .run()
    .await
}
