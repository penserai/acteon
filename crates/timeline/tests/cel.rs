mod cel_contract;

#[tokio::test]
async fn memory_cel_contract() {
    cel_contract::run(
        &acteon_timeline::memory::MemoryTimelineStore::default(),
        None,
    )
    .await;
}
