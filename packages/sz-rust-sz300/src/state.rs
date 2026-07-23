use std::sync::Arc;
use sz_orm_core::Pool;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: Arc<Pool>,
    pub pg_pool: Option<Arc<Pool>>,
}
