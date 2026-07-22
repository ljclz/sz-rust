use sz_orm_core::Pool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: Arc<Pool>,
    pub pg_pool: Option<Arc<Pool>>,
}
