BEGIN ISOLATION LEVEL REPEATABLE READ;
SET LOCAL statement_timeout = '30s';
SET LOCAL lock_timeout = '250ms';
SET LOCAL idle_in_transaction_session_timeout = '5s';
SELECT semantic_exact_qualification.exact_count(:requested_scale, :requested_channels, :recall_per_channel);
COMMIT;
