//! Step 5 proof: seeding, login, wrong password, unknown phone, lockout at five
//! failures, logout, and one outbox event per attempt in the same transaction.

mod common;

use axum::http::StatusCode;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use common::{api, call, new_id, unique_digits};
use serde_json::json;

const EVENTS: SqlStmt = sql_stmt!(
    "test.count_events_for_request",
    "sql/count_events_for_request.sql"
);
const ATTEMPT: SqlStmt = sql_stmt!(
    "test.login_attempt_for_request",
    "sql/login_attempt_for_request.sql"
);

async fn events(api: &common::Api, request_id: &str) -> String {
    let r = request_id.to_owned();
    api.state
        .db
        .run(move |s| s.query_row::<Option<String>>(&EVENTS, &[&r]))
        .await
        .expect("events")
        .unwrap_or_default()
}

async fn attempt(api: &common::Api, request_id: &str) -> (String, String) {
    let r = request_id.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&ATTEMPT, &[&r]))
        .await
        .expect("attempt")
}

#[tokio::test]
async fn login_lockout_logout_and_events() {
    let api = api().await;
    let n = unique_digits(8);
    let phone = format!("010{n}");
    let customer_no = format!("C{n}");
    let account_no = format!("9980{n}");

    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/customers", &new_id(), None, Some(json!({
        "customers": [{ "customer_no": customer_no, "name": "김테스트", "phone": phone, "address": "서울",
                        "customer_type": "INDIVIDUAL", "password": "right-pass-1" }]
    }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/accounts",
        &new_id(),
        None,
        Some(json!({
            "accounts": [{ "customer_no": customer_no, "atm_pin": common::TEST_ATM_PIN, "account_no": account_no }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["accounts"][0]["fintech_use_num"]
            .as_str()
            .map(str::len),
        Some(24)
    );

    // Seeding is internal only.
    let (status, _) = call(
        &api.public,
        "POST",
        "/internal/v1/seed/customers",
        &new_id(),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Success from a new device: login.succeeded and device.new.
    let r = new_id();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-1", "device_id": "dev-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["access_token"].as_str().expect("token").to_owned();
    assert_eq!(body["scope"], "login inquiry transfer");
    assert_eq!(body["user_seq_no"].as_str().map(str::len), Some(10));
    assert_eq!(
        events(&api, &r).await,
        "login.succeeded,device.new,request.completed"
    );
    assert_eq!(attempt(&api, &r).await, ("SUCCESS".into(), "Y".into()));

    // Same device again: no device.new.
    let r = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-1", "device_id": "dev-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events(&api, &r).await, "login.succeeded,request.completed");

    // Logout revokes the token.
    let r = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/oauth/2.0/revoke",
        &r,
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events(&api, &r).await, "logout,request.completed");
    let (status, body) = call(
        &api.generator,
        "POST",
        "/oauth/2.0/revoke",
        &new_id(),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a revoked token is rejected: {body}"
    );

    // Unknown phone: recorded without a customer.
    let r = new_id();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": format!("019{n}"), "password": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["rsp_code"], "O0001");
    assert_eq!(
        attempt(&api, &r).await,
        ("UNKNOWN_PHONE".into(), "N".into())
    );
    assert_eq!(events(&api, &r).await, "login.failed,request.completed");

    // Five wrong passwords lock the credential; the correct password is then refused.
    for i in 1..=5 {
        let r = new_id();
        let (status, body) = call(
            &api.generator,
            "POST",
            "/v2.0/ext/auth/login",
            &r,
            None,
            Some(json!({ "phone": phone, "password": "wrong" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(attempt(&api, &r).await.0, "WRONG_PASSWORD");
        assert_eq!(events(&api, &r).await, "login.failed,request.completed");
        if i == 5 {
            assert!(
                body["rsp_message"].as_str().unwrap_or("").contains("잠겼"),
                "fifth failure reports the lock"
            );
        }
    }
    let r = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &r,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(attempt(&api, &r).await.0, "LOCKED");
    assert_eq!(events(&api, &r).await, "login.failed,request.completed");
}

mod deferred_touch {
    use super::*;
    use axum::body::Body;
    use axum::extract::{FromRequestParts, Request};
    use axum::response::IntoResponse;
    use bank::shared::auth::{AuthCustomer, SESSION_IDLE, token_hash};
    use bank::shared::context::{BusinessTime, Listener, RequestContext};
    use bank::shared::db::DbError;
    use bank::shared::outbox::{self, Event};
    use bank::shared::outcome::Recorder;
    use bank::shared::telemetry::RequestStats;
    use chrono::{DateTime, Duration, FixedOffset, Timelike};

    const STATE: SqlStmt = sql_stmt!("test.auth_deferred_state", "sql/auth_deferred_state.sql");
    const PREPARE: SqlStmt = sql_stmt!(
        "test.auth_deferred_prepare",
        "sql/auth_deferred_prepare.sql"
    );
    const LOCK: SqlStmt = sql_stmt!("test.auth_deferred_lock", "sql/auth_deferred_lock.sql");
    type At = DateTime<FixedOffset>;
    type Stored = (At, At, Option<At>, Option<At>);

    async fn stored(api: &common::Api, token: &str) -> Stored {
        let hash = token_hash(token);
        api.state
            .db
            .run(move |s| s.query_row(&STATE, &[&hash]))
            .await
            .unwrap()
    }

    async fn prepare(api: &common::Api, token: &str, now: At, scope: &str) {
        let hash = token_hash(token);
        let scope = scope.to_owned();
        api.state
            .db
            .run(move |s| {
                s.execute(&PREPARE, &[&hash, &now, &scope])?;
                s.commit()
            })
            .await
            .unwrap();
    }

    async fn expiry_events(api: &common::Api, session_id: &str) -> Vec<serde_json::Value> {
        const EXPIRY_EVENTS: SqlStmt = SqlStmt {
            name: "test.auth_expiry_events",
            sql: "SELECT JSON_SERIALIZE(event_json RETURNING VARCHAR2(4000)) FROM outbox
                  WHERE JSON_VALUE(event_json, '$.event_type') = 'session.expired'
                    AND JSON_VALUE(event_json, '$.session_id') = :session_id",
        };
        let session_id = session_id.to_owned();
        let rows: Vec<String> = api
            .state
            .db
            .run(move |s| s.query_all(&EXPIRY_EVENTS, &[&session_id]))
            .await
            .unwrap();
        rows.iter()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect()
    }

    async fn end_reason(api: &common::Api, session_id: &str) -> String {
        const END_REASON: SqlStmt = SqlStmt {
            name: "test.auth_end_reason",
            sql: "SELECT end_reason FROM auth_session WHERE session_id = :session_id",
        };
        let session_id = session_id.to_owned();
        api.state
            .db
            .run(move |s| s.query_row(&END_REASON, &[&session_id]))
            .await
            .unwrap()
    }

    async fn assert_unauthorized_at(api: &common::Api, token: &str, at: At) {
        let (mut parts, _) = Request::builder()
            .header("authorization", format!("Bearer {token}"))
            .extension(BusinessTime(at))
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let error = AuthCustomer::from_request_parts(&mut parts, &api.state)
            .await
            .unwrap_err();
        assert_eq!(error.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    fn assert_touched(state: Stored, now: At) {
        assert_eq!(state, (now + Duration::minutes(30), now, None, None));
    }

    async fn assert_http_touched(api: &common::Api, token: &str, before: At) {
        let after = api.state.clock.state().await.unwrap().business_time;
        let (expires, seen, revoked, ended) = stored(api, token).await;
        assert!(seen.timestamp_micros() >= before.timestamp_micros());
        assert!(seen.timestamp_micros() <= after.timestamp_micros());
        assert_eq!(expires, seen + Duration::minutes(30));
        assert!(revoked.is_none() && ended.is_none());
    }

    struct InFlight {
        stats: RequestStats,
        recorder: Recorder,
        who: AuthCustomer,
        now: At,
    }

    async fn authenticate(api: &common::Api, token: &str, now: At) -> InFlight {
        let stats = RequestStats::default();
        let mut req = Request::builder()
            .uri("/v2.0/user/me")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let ctx = RequestContext::from_headers(Listener::Public, req.headers(), None);
        req.extensions_mut().insert(ctx.clone());
        req.extensions_mut().insert(stats.clone());
        req.extensions_mut().insert(BusinessTime(now));
        let recorder = Recorder::attach(&mut req, ctx);
        let (mut parts, _) = req.into_parts();
        let who = AuthCustomer::from_request_parts(&mut parts, &api.state)
            .await
            .unwrap();
        let collected = stats.lock().unwrap();
        assert_eq!(
            collected.count, 1,
            "authentication performs only select_token"
        );
        assert!(!collected.completion_committed);
        assert!(collected.pending_touch.is_some());
        drop(collected);
        InFlight {
            stats,
            recorder,
            who,
            now,
        }
    }

    fn completion(request: &InFlight) -> Event {
        Event::for_request(
            &request.recorder.ctx,
            "request.completed",
            request.now,
            "MOBILE",
            "completed",
        )
        .session(request.who.session_id.clone())
        .device(Some("deferred-test".into()))
    }

    async fn finish(api: &common::Api, request: &InFlight, status: StatusCode) {
        let response = request
            .recorder
            .finish(&api.state, request.now, status.into_response())
            .await;
        assert_eq!(response.status(), status);
        {
            let stats = request.stats.lock().unwrap();
            assert!(stats.completion_committed);
            assert!(stats.pending_touch.is_none());
        }
        assert_eq!(
            events(api, &request.recorder.ctx.request_id).await,
            "request.completed"
        );
    }

    #[tokio::test]
    async fn expired_bearer_materializes_once_and_deferred_touch_leaves_it_ended() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(
            &api,
            &customer.token,
            now - Duration::hours(1),
            "login inquiry transfer",
        )
        .await;
        let (expires, last_seen, _, _) = stored(&api, &customer.token).await;
        let at = expires - Duration::seconds(1);
        let inflight = authenticate(&api, &customer.token, at).await;
        let session_id = &inflight.who.session_id;
        let blocked_id = new_id();
        let (status, body) = call(
            &api.public,
            "GET",
            "/v2.0/user/me",
            &blocked_id,
            Some(&customer.token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(
            stored(&api, &customer.token).await,
            (expires, last_seen, None, None)
        );
        assert!(expiry_events(&api, session_id).await.is_empty());

        finish(&api, &inflight, StatusCode::OK).await;
        let extended = (at + SESSION_IDLE, at, None, None);
        assert_eq!(stored(&api, &customer.token).await, extended);

        let expired_at = at + SESSION_IDLE + Duration::seconds(1);
        prepare(&api, &customer.token, expired_at, "login inquiry transfer").await;
        let (_, last_seen_after, _, _) = stored(&api, &customer.token).await;
        let id = new_id();
        let (status, body) = call(
            &api.public,
            "GET",
            "/v2.0/user/me",
            &id,
            Some(&customer.token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        let effective_expiry = last_seen_after + SESSION_IDLE;
        let closed = (
            last_seen_after + SESSION_IDLE,
            last_seen_after,
            None,
            Some(effective_expiry),
        );
        assert_eq!(stored(&api, &customer.token).await, closed);
        assert_eq!(end_reason(&api, session_id).await, "expired");
        let recorded = expiry_events(&api, session_id).await;
        let [event] = recorded.as_slice() else {
            panic!("expected one expiry event, got {recorded:?}");
        };
        assert!(
            common::envelope_errors(event).is_empty(),
            "{:?}",
            common::envelope_errors(event)
        );
        assert_eq!(event["channel"], "MOBILE");
        assert_eq!(event["outcome"], "completed");
        assert_eq!(event["request_id"], id);
        assert_eq!(event["customer_no"], inflight.who.customer_no);
        assert_eq!(event["device_id"], "payee-test");
        assert_eq!(
            event["payload"],
            json!({
                "session_id": session_id,
                "ended_at": effective_expiry.to_rfc3339(),
                "end_reason": "expired",
            })
        );

        let retry_id = new_id();
        let (status, _) = call(
            &api.public,
            "GET",
            "/v2.0/user/me",
            &retry_id,
            Some(&customer.token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_unauthorized_at(&api, &customer.token, expired_at).await;
        assert_eq!(stored(&api, &customer.token).await, closed);
        assert_eq!(expiry_events(&api, session_id).await, recorded);
        assert_eq!(events(&api, &retry_id).await, "request.completed");
    }

    #[tokio::test]
    async fn authentication_defers_until_completion_and_rollback_retains_touch() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(&api, &customer.token, now, "login inquiry transfer").await;
        let before = stored(&api, &customer.token).await;
        let request = authenticate(&api, &customer.token, now).await;
        assert_eq!(stored(&api, &customer.token).await, before);
        api.state
            .db
            .run_in(&request.stats, |s| {
                s.note_completion();
                s.rollback()?;
                s.commit() // unrelated commit must neither apply nor clear activity
            })
            .await
            .unwrap();
        assert_eq!(stored(&api, &customer.token).await, before);
        assert!(request.stats.lock().unwrap().pending_touch.is_some());
        let event = completion(&request);
        api.state
            .db
            .run_in(&request.stats, move |s| {
                outbox::record(s, &event)?;
                s.commit()?;
                assert_eq!(s.stats.count, 2, "outbox insert and one touch block");
                s.commit()?; // completion is no longer pending within this session
                assert_eq!(s.stats.count, 2);
                Ok(())
            })
            .await
            .unwrap();
        assert_touched(stored(&api, &customer.token).await, now);
        let count = request.stats.lock().unwrap().count;
        finish(&api, &request, StatusCode::OK).await;
        assert_eq!(
            request.stats.lock().unwrap().count,
            count,
            "finish skips committed completion"
        );
    }

    #[test]
    fn metadata_survives_unrelated_work_and_debug_redacts_token() {
        use bank::shared::auth::PendingTouch;
        use bank::shared::db::StatementStats;
        let mut stats = StatementStats {
            pending_touch: Some(PendingTouch::detached(
                "sensitive-token-hash-sentinel".into(),
                "session".into(),
                DateTime::parse_from_rfc3339("2026-11-17T12:00:00+09:00").unwrap(),
            )),
            ..Default::default()
        };
        stats.merge(&StatementStats::default());
        assert!(stats.pending_touch.is_some());
        assert!(!stats.completion_committed);
        assert!(!format!("{stats:?}").contains("sensitive-token-hash-sentinel"));
        stats.merge(&StatementStats {
            completion_committed: true,
            ..Default::default()
        });
        assert!(stats.completion_committed);
        assert!(stats.pending_touch.is_none());
    }

    #[tokio::test]
    async fn rollback_and_completion_in_same_session_retain_activity() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(&api, &customer.token, now, "login inquiry transfer").await;
        let request = authenticate(&api, &customer.token, now).await;
        let event = completion(&request);
        api.state
            .db
            .run_in(&request.stats, move |s| {
                outbox::record(s, &event)?;
                s.rollback()?;
                outbox::record(s, &event)?;
                s.commit()?;
                assert_eq!(s.stats.count, 3, "two insert attempts and one touch block");
                Ok(())
            })
            .await
            .unwrap();
        assert_touched(stored(&api, &customer.token).await, now);
        finish(&api, &request, StatusCode::OK).await;
    }

    #[tokio::test]
    async fn rejected_http_requests_and_replay_persist_activity() {
        let api = api().await;
        let customer = common::customer(&api, 1000).await;
        let recipient = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        for (path, scope, expected) in [
            ("/v2.0/user/me", "inquiry", StatusCode::FORBIDDEN),
            (
                "/v2.0/account/balance/fin_num?fintech_use_num=%FF",
                "login inquiry transfer",
                StatusCode::BAD_REQUEST,
            ),
        ] {
            prepare(&api, &customer.token, now, scope).await;
            let id = new_id();
            let before = api.state.clock.state().await.unwrap().business_time;
            let answer = call(&api.public, "GET", path, &id, Some(&customer.token), None).await;
            assert_eq!(answer.0, expected, "{}", answer.1);
            assert_http_touched(&api, &customer.token, before).await;
            assert_eq!(events(&api, &id).await, "request.completed");
        }
        let body = json!({
            "bank_tran_id": common::bank_tran_id(), "fintech_use_num": customer.fin,
            "tran_amt": "100", "tran_dtime": now.format("%Y%m%d%H%M%S").to_string(),
            "transfer_purpose": "TR", "recv_client_name": "수취인",
            "recv_client_bank_code": "998", "recv_client_account_num": recipient.account,
        });
        for replay in [false, true] {
            prepare(&api, &customer.token, now, "login inquiry transfer").await;
            let id = new_id();
            let before = api.state.clock.state().await.unwrap().business_time;
            let answer = call(
                &api.public,
                "POST",
                "/v2.0/transfer/withdraw/fin_num",
                &id,
                Some(&customer.token),
                Some(body.clone()),
            )
            .await;
            assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
            assert_http_touched(&api, &customer.token, before).await;
            assert_eq!(
                events(&api, &id)
                    .await
                    .split(',')
                    .filter(|e| *e == "request.completed")
                    .count(),
                1
            );
            if replay {
                assert_eq!(events(&api, &id).await, "request.completed");
            }
        }
    }

    #[tokio::test]
    async fn older_completion_cannot_regress_activity_and_logout_is_not_revived() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(&api, &customer.token, now, "login inquiry transfer").await;
        let older = authenticate(&api, &customer.token, now + Duration::minutes(5)).await;
        let newer = authenticate(&api, &customer.token, now + Duration::minutes(10)).await;
        finish(&api, &newer, StatusCode::OK).await;
        finish(&api, &older, StatusCode::OK).await;
        assert_touched(stored(&api, &customer.token).await, newer.now);
        let stale = authenticate(&api, &customer.token, now + Duration::minutes(15)).await;
        let before = stored(&api, &customer.token).await;
        let id = new_id();
        let answer = call(
            &api.public,
            "POST",
            "/oauth/2.0/revoke",
            &id,
            Some(&customer.token),
            None,
        )
        .await;
        assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);
        let logged_out = stored(&api, &customer.token).await;
        assert_eq!((logged_out.0, logged_out.1), (before.0, before.1));
        assert!(logged_out.2.is_some() && logged_out.3.is_some());
        finish(&api, &stale, StatusCode::OK).await;
        assert_eq!(stored(&api, &customer.token).await, logged_out);
        assert_eq!(events(&api, &id).await, "logout,request.completed");
        assert_unauthorized_at(&api, &customer.token, logged_out.0 + SESSION_IDLE).await;
        assert_eq!(end_reason(&api, &stale.who.session_id).await, "LOGOUT");
        assert!(expiry_events(&api, &stale.who.session_id).await.is_empty());
    }

    #[tokio::test]
    async fn concurrent_http_requests_sharing_token_both_complete() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(&api, &customer.token, now, "login inquiry transfer").await;
        let first = new_id();
        let second = new_id();
        let before = api.state.clock.state().await.unwrap().business_time;
        let (a, b) = tokio::join!(
            call(
                &api.public,
                "GET",
                "/v2.0/user/me",
                &first,
                Some(&customer.token),
                None
            ),
            call(
                &api.public,
                "GET",
                "/v2.0/user/me",
                &second,
                Some(&customer.token),
                None
            ),
        );
        assert_eq!(a.0, StatusCode::OK, "{}", a.1);
        assert_eq!(b.0, StatusCode::OK, "{}", b.1);
        assert_http_touched(&api, &customer.token, before).await;
        assert_eq!(events(&api, &first).await, "request.completed");
        assert_eq!(events(&api, &second).await, "request.completed");
    }

    #[tokio::test]
    async fn password_reset_does_not_revive_in_flight_activity() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(&api, &customer.token, now, "login inquiry transfer").await;
        let request = authenticate(&api, &customer.token, now).await;
        let before = stored(&api, &customer.token).await;
        let hash = token_hash(&customer.token);
        let phone: String = api
            .state
            .db
            .run(move |s| {
                s.query_row(
                    &sql_stmt!("test.auth_deferred_phone", "sql/auth_deferred_phone.sql"),
                    &[&hash],
                )
            })
            .await
            .unwrap();
        let (status, body) = call(
            &api.public,
            "POST",
            "/v2.0/ext/auth/password_reset/request",
            &new_id(),
            None,
            Some(json!({"phone": phone})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, sms) = call(
            &api.generator,
            "GET",
            &format!("/internal/v1/sms/{phone}"),
            &new_id(),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = call(
            &api.public,
            "POST",
            "/v2.0/ext/auth/password_reset/confirm",
            &new_id(),
            None,
            Some(
                json!({"phone": phone, "code": sms["code"], "new_password": "deferred-reset-pass"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let reset = stored(&api, &customer.token).await;
        assert!(reset.2.is_some() && reset.3.is_some());
        assert_eq!((reset.0, reset.1), (before.0, before.1));
        finish(&api, &request, StatusCode::OK).await;
        assert_eq!(stored(&api, &customer.token).await, reset);
        assert_unauthorized_at(&api, &customer.token, reset.0 + SESSION_IDLE).await;
        assert_eq!(
            end_reason(&api, &request.who.session_id).await,
            "password_reset"
        );
        assert!(
            expiry_events(&api, &request.who.session_id)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn timed_out_touch_rolls_back_completion_then_finish_retries() {
        let api = api().await;
        let customer = common::customer(&api, 0).await;
        let now = api
            .state
            .clock
            .state()
            .await
            .unwrap()
            .business_time
            .with_nanosecond(0)
            .unwrap();
        prepare(&api, &customer.token, now, "login inquiry transfer").await;
        let before = stored(&api, &customer.token).await;
        let request = authenticate(&api, &customer.token, now).await;
        let db = api.state.db.clone();
        let hash = token_hash(&customer.token);
        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = tokio::spawn(async move {
            db.run(move |s| {
                let _: String = s.query_row(&LOCK, &[&hash])?;
                locked_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(15))
                    .unwrap();
                s.rollback()
            })
            .await
        });
        locked_rx.await.unwrap();
        let event = completion(&request);
        let result = api
            .state
            .db
            .run_in(&request.stats, move |s| {
                outbox::record(s, &event)?;
                s.commit()
            })
            .await;
        release_tx.send(()).unwrap();
        holder.await.unwrap().unwrap();
        assert!(matches!(result, Err(DbError::Busy)));
        assert_eq!(stored(&api, &customer.token).await, before);
        assert!(!request.stats.lock().unwrap().completion_committed);
        assert!(request.stats.lock().unwrap().pending_touch.is_some());
        assert!(
            events(&api, &request.recorder.ctx.request_id)
                .await
                .is_empty()
        );
        finish(&api, &request, StatusCode::SERVICE_UNAVAILABLE).await;
        assert_touched(stored(&api, &customer.token).await, now);
    }
}

#[tokio::test]
async fn concurrent_expired_lock_logins_relock_after_five_failures() {
    const EXPIRE_LOCK: SqlStmt = SqlStmt {
        name: "test.auth_expire_lock",
        sql: "UPDATE customer_credential \
              SET failed_count = 5, locked_until = :expired_until \
              WHERE customer_id = (SELECT customer_id FROM customer WHERE customer_no = :customer_no)",
    };

    let api = api().await;
    let n = unique_digits(8);
    let phone = format!("010{n}");
    let customer_no = format!("C{n}");

    let (status, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/customers",
        &new_id(),
        None,
        Some(json!({
            "customers": [{
                "customer_no": customer_no,
                "name": "동시잠금",
                "phone": phone,
                "address": "서울",
                "customer_type": "INDIVIDUAL",
                "password": "right-pass-concurrent"
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Place the credential into an expired-lock state relative to business time
    // without mutating the shared clock.
    let expired_until =
        api.state.clock.state().await.unwrap().business_time - chrono::Duration::minutes(1);
    let cno = customer_no.clone();
    api.state
        .db
        .run(move |s| {
            let updated = s.execute(&EXPIRE_LOCK, &[&expired_until, &cno])?;
            assert_eq!(updated, 1);
            s.commit()
        })
        .await
        .expect("set expired lock");

    // Six concurrent wrong-password logins after lock expiry must serialize via FOR UPDATE,
    // reset once, count 1..=5 to re-lock, and reject the sixth as LOCKED.
    let mut handles = Vec::new();
    for _ in 0..6 {
        let router = api.generator.clone();
        let phone = phone.clone();
        handles.push(tokio::spawn(async move {
            let rid = new_id();
            let (status, body) = call(
                &router,
                "POST",
                "/v2.0/ext/auth/login",
                &rid,
                None,
                Some(json!({ "phone": phone, "password": "wrong" })),
            )
            .await;
            (rid, status, body)
        }));
    }

    let mut wrong_count = 0;
    let mut locked_count = 0;
    for handle in handles {
        let (rid, status, _body) = handle.await.expect("join concurrent login");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        match attempt(&api, &rid).await.0.as_str() {
            "WRONG_PASSWORD" => wrong_count += 1,
            "LOCKED" => locked_count += 1,
            other => panic!("unexpected login attempt outcome: {other}"),
        }
    }
    assert_eq!(
        wrong_count, 5,
        "exactly five failures should count toward re-lock"
    );
    assert_eq!(
        locked_count, 1,
        "sixth concurrent failure must see the active lock"
    );

    // A correct password immediately afterwards must be rejected as LOCKED.
    let rid = new_id();
    let (status, _) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &rid,
        None,
        Some(json!({ "phone": phone, "password": "right-pass-concurrent" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(attempt(&api, &rid).await.0, "LOCKED");
}
