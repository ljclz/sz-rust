use serde_json::{json, Value};
use sz_rust_core::orm::repository::Repository;
use sz_rust_core::orm::Value as OrmValue;

use crate::model::board::Board;

pub struct BoardController;

impl BoardController {
    pub async fn list<R: Repository<Board, Key = OrmValue>>(repo: &R) -> Value {
        match repo.paginate_by(&[], 1, 10000) {
            Ok(pr) => json!({"code": 0, "msg": "ok", "data": pr.items}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Board, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut board: Board = match serde_json::from_value(body) {
            Ok(b) => b,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if board.name.is_empty() {
            return json!({"code": 400, "msg": "name is required", "data": null});
        }
        board.id = 0;
        match repo.save(board) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}
