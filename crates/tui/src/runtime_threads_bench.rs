#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::tempdir;

    #[tokio::test]
    async fn bench_get_thread_detail() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let store = Arc::new(RuntimeThreadStore::open(dir.path().to_path_buf())?);

        let thread_id = "thread_bench_1".to_string();
        store.save_thread(&ThreadRecord {
            schema_version: 2,
            id: thread_id.clone(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            model: "test".to_string(),
            workspace: dir.path().to_path_buf(),
            mode: "test".to_string(),
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            latest_turn_id: None,
            latest_response_bookmark: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            title: None,
            coherence_state: CoherenceState::Optimal,
        })?;

        for i in 0..100 {
            let turn_id = format!("turn_{}", i);
            store.save_turn(&TurnRecord {
                schema_version: 2,
                id: turn_id.clone(),
                thread_id: thread_id.clone(),
                status: RuntimeTurnStatus::Completed,
                input_summary: "test".to_string(),
                created_at: chrono::Utc::now(),
                started_at: None,
                ended_at: None,
                duration_ms: None,
                usage: None,
                error: None,
                item_ids: vec![],
                steer_count: 0,
            })?;

            for j in 0..10 {
                let item_id = format!("item_{}_{}", i, j);
                store.save_item(&TurnItemRecord {
                    schema_version: 2,
                    id: item_id,
                    turn_id: turn_id.clone(),
                    kind: TurnItemKind::UserMessage,
                    status: TurnItemLifecycleStatus::Completed,
                    summary: "test".to_string(),
                    detail: None,
                    metadata: None,
                    artifact_refs: vec![],
                    started_at: None,
                    ended_at: None,
                })?;
            }
        }

        let manager = RuntimeManager::new(dir.path().to_path_buf(), false, None)?;

        let start = Instant::now();
        let detail = manager.get_thread_detail(&thread_id).await?;
        let elapsed = start.elapsed();

        println!("Thread turns: {}, Thread items: {}", detail.turns.len(), detail.items.len());
        println!("Elapsed time before fix: {:?}", elapsed);
        std::fs::write("perf_output.txt", format!("{:?}", elapsed))?;

        Ok(())
    }
}
