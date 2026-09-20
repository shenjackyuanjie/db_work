// 契约表分组：商城与交易。
// 对应 Django 的 CitrusProduct / ProductImage / BuyerAddress / CartItem / Order /
// OrderItem / PaymentRecord / TracePackage / AfterSaleRequest 共 9 个模型。
// 拆成两个切片的原因：`citrus_product` 必须早于 trace_tables::QUALITY_DDL（品质抽检要引用商品），
// 而 `order` 及其下游必须晚于 sales_batch 与 citrus_product（见 bootstrap.rs 的执行顺序注释）。
// 注意：`order` 是 PostgreSQL 保留字，建表与引用都必须双引号包裹。

pub(super) const PRODUCT_DDL: &[&str] = &[
    // CitrusProduct -> citrus_product
    r#"CREATE TABLE IF NOT EXISTS citrus_product (
            id UUID PRIMARY KEY,
            seller_id UUID NULL REFERENCES "user"(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
            sales_batch_id UUID NULL REFERENCES sales_batch(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            name VARCHAR(120) NOT NULL,
            sku_type VARCHAR(20) NOT NULL DEFAULT 'family' CHECK (sku_type IN ('trial', 'family', 'gift', 'juice', 'enterprise', 'specialty')),
            fruit_type VARCHAR(40) NOT NULL DEFAULT '脐橙',
            variety VARCHAR(80) NOT NULL DEFAULT '纽荷尔脐橙',
            origin VARCHAR(160) NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            price NUMERIC(10, 2) NOT NULL,
            unit VARCHAR(30) NOT NULL DEFAULT '5斤/箱',
            stock INTEGER NOT NULL DEFAULT 0 CHECK (stock >= 0),
            sweetness NUMERIC(4, 1) NULL,
            grade VARCHAR(40) NOT NULL DEFAULT '',
            harvest_date DATE NULL,
            shipping_note VARCHAR(160) NOT NULL DEFAULT '',
            cover_image_url VARCHAR(500) NOT NULL DEFAULT '',
            purchase_limit INTEGER NOT NULL DEFAULT 20 CHECK (purchase_limit >= 0),
            minimum_order_quantity INTEGER NOT NULL DEFAULT 1 CHECK (minimum_order_quantity >= 0),
            sort_order INTEGER NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
            status VARCHAR(20) NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'on_sale', 'off_sale')),
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_citrus_product_seller_id ON citrus_product(seller_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_citrus_product_sales_batch_id ON citrus_product(sales_batch_id)"#,
    // ProductImage -> product_image
    r#"CREATE TABLE IF NOT EXISTS product_image (
            id UUID PRIMARY KEY,
            product_id UUID NOT NULL REFERENCES citrus_product(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            image_url VARCHAR(500) NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0 CHECK (sort_order >= 0)
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_product_image_product_id ON product_image(product_id)"#,
];

pub(super) const DDL: &[&str] = &[
    // BuyerAddress -> buyer_address
    r#"CREATE TABLE IF NOT EXISTS buyer_address (
            id UUID PRIMARY KEY,
            buyer_id UUID NOT NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            recipient_name VARCHAR(80) NOT NULL,
            phone VARCHAR(30) NOT NULL,
            province VARCHAR(60) NOT NULL DEFAULT '',
            city VARCHAR(60) NOT NULL DEFAULT '',
            district VARCHAR(60) NOT NULL DEFAULT '',
            detail VARCHAR(255) NOT NULL,
            is_default BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_buyer_address_buyer_id ON buyer_address(buyer_id)"#,
    // CartItem -> cart_item（买家 + 商品唯一）
    r#"CREATE TABLE IF NOT EXISTS cart_item (
            id UUID PRIMARY KEY,
            buyer_id UUID NOT NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            product_id UUID NOT NULL REFERENCES citrus_product(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            quantity INTEGER NOT NULL DEFAULT 1 CHECK (quantity >= 0),
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL,
            CONSTRAINT unique_buyer_cart_product UNIQUE (buyer_id, product_id)
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_cart_item_buyer_id ON cart_item(buyer_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_cart_item_product_id ON cart_item(product_id)"#,
    // Order -> order
    r#"CREATE TABLE IF NOT EXISTS "order" (
            id UUID PRIMARY KEY,
            order_number VARCHAR(32) NOT NULL UNIQUE,
            buyer_id UUID NOT NULL REFERENCES "user"(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            sales_batch_id UUID NULL REFERENCES sales_batch(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            status VARCHAR(24) NOT NULL DEFAULT 'pending_payment' CHECK (status IN ('pending_payment', 'pending_deposit', 'pending_balance', 'paid', 'picking', 'packed', 'shipped', 'completed', 'after_sale', 'cancelled')),
            payment_mode VARCHAR(24) NOT NULL DEFAULT 'full' CHECK (payment_mode IN ('full', 'deposit_balance')),
            recipient_name VARCHAR(80) NOT NULL,
            recipient_phone VARCHAR(30) NOT NULL,
            shipping_address VARCHAR(500) NOT NULL,
            total_amount NUMERIC(12, 2) NOT NULL,
            deposit_amount NUMERIC(12, 2) NOT NULL DEFAULT 0,
            balance_amount NUMERIC(12, 2) NOT NULL DEFAULT 0,
            paid_amount NUMERIC(12, 2) NOT NULL DEFAULT 0,
            note VARCHAR(255) NOT NULL DEFAULT '',
            expires_at TIMESTAMPTZ NOT NULL,
            balance_due_at TIMESTAMPTZ NULL,
            paid_at TIMESTAMPTZ NULL,
            cancelled_at TIMESTAMPTZ NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_order_buyer_id ON "order"(buyer_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_order_sales_batch_id ON "order"(sales_batch_id)"#,
    // OrderItem -> order_item（无 created_at）
    r#"CREATE TABLE IF NOT EXISTS order_item (
            id UUID PRIMARY KEY,
            order_id UUID NOT NULL REFERENCES "order"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            product_id UUID NULL REFERENCES citrus_product(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
            product_name VARCHAR(120) NOT NULL,
            product_image_url VARCHAR(500) NOT NULL DEFAULT '',
            batch_code VARCHAR(40) NOT NULL DEFAULT '',
            batch_title VARCHAR(160) NOT NULL DEFAULT '',
            sku_type VARCHAR(20) NOT NULL DEFAULT '',
            unit_price NUMERIC(10, 2) NOT NULL,
            unit VARCHAR(30) NOT NULL,
            quantity INTEGER NOT NULL CHECK (quantity >= 0),
            subtotal NUMERIC(12, 2) NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_order_item_order_id ON order_item(order_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_order_item_product_id ON order_item(product_id)"#,
    // PaymentRecord -> payment_record（payment_number 默认值是 Python 函数，SQL 不设默认）
    r#"CREATE TABLE IF NOT EXISTS payment_record (
            id UUID PRIMARY KEY,
            payment_number VARCHAR(32) NOT NULL UNIQUE,
            order_id UUID NOT NULL REFERENCES "order"(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            stage VARCHAR(20) NOT NULL CHECK (stage IN ('full', 'deposit', 'balance', 'refund')),
            provider VARCHAR(30) NOT NULL DEFAULT 'mock',
            amount NUMERIC(12, 2) NOT NULL,
            status VARCHAR(20) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'succeeded', 'failed', 'refunded')),
            provider_transaction_id VARCHAR(120) NOT NULL DEFAULT '',
            paid_at TIMESTAMPTZ NULL,
            raw_callback JSONB NOT NULL DEFAULT '{}'::jsonb,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_payment_record_order_id ON payment_record(order_id)"#,
    // TracePackage -> trace_package（trace_code 默认值是 Python 函数，SQL 不设默认）
    r#"CREATE TABLE IF NOT EXISTS trace_package (
            id UUID PRIMARY KEY,
            trace_code VARCHAR(40) NOT NULL UNIQUE,
            order_id UUID NOT NULL REFERENCES "order"(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            order_item_id UUID NULL REFERENCES order_item(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
            batch_id UUID NOT NULL REFERENCES sales_batch(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            sequence INTEGER NOT NULL DEFAULT 1 CHECK (sequence >= 0),
            box_spec VARCHAR(80) NOT NULL DEFAULT '',
            status VARCHAR(20) NOT NULL DEFAULT 'created' CHECK (status IN ('created', 'packed', 'shipped', 'signed', 'after_sale')),
            carrier VARCHAR(80) NOT NULL DEFAULT '',
            tracking_number VARCHAR(120) NOT NULL DEFAULT '',
            packed_at TIMESTAMPTZ NULL,
            shipped_at TIMESTAMPTZ NULL,
            signed_at TIMESTAMPTZ NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL,
            CONSTRAINT unique_order_package_sequence UNIQUE (order_id, sequence)
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_trace_package_order_id ON trace_package(order_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_trace_package_order_item_id ON trace_package(order_item_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_trace_package_batch_id ON trace_package(batch_id)"#,
    // AfterSaleRequest -> after_sale_request
    r#"CREATE TABLE IF NOT EXISTS after_sale_request (
            id UUID PRIMARY KEY,
            order_id UUID NOT NULL REFERENCES "order"(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            package_id UUID NULL REFERENCES trace_package(id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
            buyer_id UUID NOT NULL REFERENCES "user"(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
            issue_type VARCHAR(20) NOT NULL CHECK (issue_type IN ('damaged', 'spoiled', 'weight', 'logistics', 'other')),
            description TEXT NOT NULL,
            evidence_urls JSONB NOT NULL DEFAULT '[]'::jsonb,
            status VARCHAR(20) NOT NULL DEFAULT 'submitted' CHECK (status IN ('submitted', 'reviewing', 'resolved', 'rejected')),
            resolution TEXT NOT NULL DEFAULT '',
            refund_amount NUMERIC(12, 2) NOT NULL DEFAULT 0,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_after_sale_request_order_id ON after_sale_request(order_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_after_sale_request_package_id ON after_sale_request(package_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_after_sale_request_buyer_id ON after_sale_request(buyer_id)"#,
];
