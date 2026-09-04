use super::{IndexedSessionRepository, RepositoryError};

impl IndexedSessionRepository {
    /// Clears app-owned library state while retaining the migrated schema and
    /// the open hardened pool. Source artifacts are outside this boundary.
    pub(crate) async fn reset_local_database(&self) -> Result<(), RepositoryError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;

        sqlx::query("PRAGMA secure_delete = ON")
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;

        sqlx::query("DELETE FROM sessions")
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        sqlx::query("DELETE FROM suppressed_sources")
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        sqlx::query("DELETE FROM scan_runs")
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;

        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        // The user-visible operation is complete once the transaction commits.
        // Best-effort compaction must never turn that committed reset into a
        // reported rollback. `secure_delete` above clears deleted cells on the
        // writer connection; these steps reduce residual free/WAL pages when
        // no concurrent read transaction is holding them.
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("VACUUM").execute(&self.pool).await;
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await;
        Ok(())
    }
}
