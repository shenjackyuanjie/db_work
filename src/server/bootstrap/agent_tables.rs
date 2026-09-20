// 契约表分组：智能体审批与反馈。
// 对应 Django 的 AgentApproval / AgentFeedback 共 2 个模型。
// 两张表都只依赖 `user`，因此在 bootstrap 流程的最后执行。

pub(super) const DDL: &[&str] = &[
    // AgentApproval -> agent_approval
    r#"CREATE TABLE IF NOT EXISTS agent_approval (
            id UUID PRIMARY KEY,
            ticket_type VARCHAR(24) NOT NULL DEFAULT 'other' CHECK (ticket_type IN ('quote', 'risk_action', 'restock', 'other')),
            title VARCHAR(200) NOT NULL,
            ref_type VARCHAR(50) NOT NULL DEFAULT '',
            ref_id VARCHAR(64) NOT NULL DEFAULT '',
            payload JSONB NOT NULL DEFAULT '{}'::jsonb,
            status VARCHAR(20) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
            created_by_id UUID NULL REFERENCES "user"(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
            decided_by_id UUID NULL REFERENCES "user"(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
            note TEXT NOT NULL DEFAULT '',
            created_at TIMESTAMPTZ NOT NULL,
            decided_at TIMESTAMPTZ NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_agent_approval_created_by_id ON agent_approval(created_by_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_agent_approval_decided_by_id ON agent_approval(decided_by_id)"#,
    // AgentFeedback -> agent_feedback（rating 为 PositiveSmallIntegerField，Django 只加 >= 0 约束）
    r#"CREATE TABLE IF NOT EXISTS agent_feedback (
            id UUID PRIMARY KEY,
            user_id UUID NOT NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            product_name VARCHAR(120) NOT NULL DEFAULT '',
            batch_code VARCHAR(40) NOT NULL DEFAULT '',
            rating SMALLINT NOT NULL DEFAULT 4 CHECK (rating >= 0),
            taste VARCHAR(200) NOT NULL DEFAULT '',
            package VARCHAR(200) NOT NULL DEFAULT '',
            note TEXT NOT NULL DEFAULT '',
            created_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_agent_feedback_user_id ON agent_feedback(user_id)"#,
];
