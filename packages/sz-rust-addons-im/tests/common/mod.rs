use std::sync::Arc;
use sz_rust_addons_im::model::conversation::Conversation;
use sz_rust_addons_im::model::message::Message;
use sz_rust_addons_im::model::user_status::UserStatus;
use sz_rust_core::orm::repository::InMemoryRepository;

pub fn conversation_repo() -> Arc<InMemoryRepository<Conversation>> {
    Arc::new(InMemoryRepository::new())
}

pub fn message_repo() -> Arc<InMemoryRepository<Message>> {
    Arc::new(InMemoryRepository::new())
}

pub fn user_status_repo() -> Arc<InMemoryRepository<UserStatus>> {
    Arc::new(InMemoryRepository::new())
}
