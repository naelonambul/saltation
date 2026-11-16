SELECT stamped, TO_CHAR(stamped, 'YYYY-MM-DD HH24:MI:SS.FF6 TZH:TZM'), TO_CHAR(SYS_EXTRACT_UTC(stamped), 'YYYY-MM-DD HH24:MI:SS.FF6'), default_stamp FROM driver_parity_t WHERE key_value = :key_value
