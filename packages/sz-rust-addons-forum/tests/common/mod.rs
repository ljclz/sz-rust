use std::sync::Arc;
use sz_rust_addons_forum::model::board::Board;
use sz_rust_addons_forum::model::reply::Reply;
use sz_rust_addons_forum::model::topic::Topic;
use sz_rust_core::orm::repository::InMemoryRepository;

pub fn forum_state() -> sz_rust_addons_forum::ForumState {
    sz_rust_addons_forum::ForumState::default()
}

pub fn board_repo() -> Arc<InMemoryRepository<Board>> {
    Arc::new(InMemoryRepository::new())
}

pub fn topic_repo() -> Arc<InMemoryRepository<Topic>> {
    Arc::new(InMemoryRepository::new())
}

pub fn reply_repo() -> Arc<InMemoryRepository<Reply>> {
    Arc::new(InMemoryRepository::new())
}
