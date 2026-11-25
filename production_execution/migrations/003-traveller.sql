ALTER TABLE factory_production_orders
 ADD COLUMN traveller jsonb NOT NULL CHECK (jsonb_typeof(traveller) = 'object');
