ALTER TABLE implementation_attempts ADD COLUMN paused_at INTEGER;
ALTER TABLE implementation_attempts ADD COLUMN parent_attempt_id TEXT
    REFERENCES implementation_attempts(id) ON DELETE RESTRICT;
ALTER TABLE implementation_attempts ADD COLUMN continuation_kind TEXT CHECK (
    continuation_kind IS NULL OR continuation_kind IN ('redirect', 'additional_context')
);
ALTER TABLE implementation_attempts ADD COLUMN user_instruction TEXT CHECK (
    user_instruction IS NULL OR length(trim(user_instruction)) > 0
);
ALTER TABLE implementation_attempts ADD COLUMN stop_reason TEXT CHECK (
    stop_reason IS NULL OR stop_reason IN ('cancelled', 'redirected', 'context_added')
);
ALTER TABLE implementation_attempts ADD COLUMN pending_continuation_kind TEXT CHECK (
    pending_continuation_kind IS NULL OR pending_continuation_kind IN ('redirect', 'additional_context')
);
ALTER TABLE implementation_attempts ADD COLUMN pending_user_instruction TEXT CHECK (
    pending_user_instruction IS NULL OR length(trim(pending_user_instruction)) > 0
);

CREATE INDEX implementation_attempts_parent_idx
    ON implementation_attempts(parent_attempt_id);
