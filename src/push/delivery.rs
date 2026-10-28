use super::{
    PushSource,
    fcm::{PushSender, SendResult},
    models::{PushDelivery, PushDevice},
};
use che_orm::{Database, DatabaseValue, OrmError, Uuid};
use std::sync::Arc;
use time::OffsetDateTime;
pub async fn capture(db: &Database, source: Arc<dyn PushSource>) -> Result<(), OrmError> {
    db.transaction(move |tx| {
        Box::pin(async move {
            let events = source.capture(tx).await?;
            if events.is_empty() {
                return Ok(());
            }
            let devices = tx.query::<PushDevice>().all().await?;
            for event in events {
                let data = serde_json::to_string(&event.data)
                    .map_err(|_| OrmError::Configuration("Invalid push data".into()))?;
                let content = serde_json::json!({"data":event.data,"title":event.title,"body":event.body,"kind":event.kind,"target_type":event.target_type,"target_id":event.target_id});
                if event.key.is_empty() || content.to_string().len() > 2500
                {
                    return Err(OrmError::Configuration(
                        "Push event exceeds payload budget".into(),
                    ));
                }
                for device in devices.iter().filter(|d| {
                    d.enabled
                        && event.users.contains(&d.user_id)
                        && d.registered_at <= event.created_at
                }) {
                    let key = serde_json::to_string(&(
                        &event.key,
                        &event.target_type,
                        &event.target_id,
                        device.id,
                        device.generation,
                    ))
                    .map_err(|_| OrmError::Configuration("Invalid push key".into()))?;
                    if tx
                        .query::<PushDelivery>()
                        .filter(PushDelivery::DELIVERY_KEY.eq(key.clone()))
                        .first()
                        .await?
                        .is_some()
                    {
                        continue;
                    }
                    tx.create::<PushDelivery>()
                        .set(PushDelivery::DELIVERY_KEY, key)
                        .set(PushDelivery::DEVICE_ID, device.id)
                        .set(PushDelivery::DEVICE_GENERATION, device.generation)
                        .set(PushDelivery::USER_ID, device.user_id)
                        .set(PushDelivery::TARGET_TYPE, event.target_type.clone())
                        .set(PushDelivery::TARGET_ID, event.target_id.clone())
                        .set(PushDelivery::TITLE, event.title.clone())
                        .set(PushDelivery::BODY, event.body.clone())
                        .set(PushDelivery::DATA, data.clone())
                        .set(PushDelivery::KIND, event.kind.clone())
                        .set(PushDelivery::STATE, "pending")
                        .set(PushDelivery::ATTEMPTS, 0_i64)
                        .set(PushDelivery::NEXT_ATTEMPT_AT, 0_i64)
                        .set(PushDelivery::LEASE_UNTIL, 0_i64)
                        .set(PushDelivery::LEASE_TOKEN, "")
                        .set(PushDelivery::LAST_ERROR, "")
                        .set(PushDelivery::CREATED_AT, event.created_at)
                        .execute()
                        .await?;
                }
            }
            Ok(())
        })
    })
    .await
}
pub async fn drain(
    db: &Database,
    sender: &dyn PushSender,
    source: std::sync::Arc<dyn super::PushSource>,
) -> Result<(), OrmError> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let rows=db.query_sql("SELECT id FROM push_deliveries WHERE state IN ('pending','sending') AND next_attempt_at<=$1 AND lease_until<=$1 ORDER BY created_at,id LIMIT 50",vec![DatabaseValue::Integer(now)]).await?;
    for row in rows {
        let id = row.get::<Uuid>(0)?;
        let source = source.clone();
        let claimed = db
            .transaction(move |tx| {
                Box::pin(async move {
                    let Some(mut job) = tx.get::<PushDelivery>(id).await? else {
                        return Ok(None);
                    };
                    if !matches!(job.state.as_str(), "pending" | "sending")
                        || job.lease_until > now
                        || job.next_attempt_at > now
                    {
                        return Ok(None);
                    }
                    let device = tx.get::<PushDevice>(job.device_id).await?;
                    let device = device.filter(|device| {
                        device.enabled
                            && device.generation == job.device_generation
                            && device.user_id == job.user_id
                    });
                    let relevant = source.relevant(tx, &job).await?;
                    if !relevant
                        || device.is_none()
                        || job.created_at.unix_timestamp() < now - 86400 * 2
                    {
                        job.state = "cancelled".into();
                        tx.save(&job).await?;
                        return Ok(None);
                    }
                    job.state = "sending".into();
                    job.attempts += 1;
                    job.lease_until = now + 60;
                    job.lease_token = Uuid::now_v7().to_string();
                    tx.save(&job).await?;
                    Ok(Some((job, device.unwrap())))
                })
            })
            .await?;
        let Some((job, device)) = claimed else {
            continue;
        };
        let result = sender.send(&device, &job).await;
        db.transaction(move |tx| {
            Box::pin(async move {
                let Some(mut current) = tx.get::<PushDelivery>(job.id).await? else {
                    return Ok(());
                };
                if current.lease_token != job.lease_token {
                    return Ok(());
                }
                current.lease_until = 0;
                match result {
                    SendResult::Accepted => {
                        current.state = "delivered".into();
                        current.last_error.clear();
                    }
                    SendResult::Retry(reason, retry) => {
                        current.state = if current.attempts >= 12 {
                            "failed"
                        } else {
                            "pending"
                        }
                        .into();
                        current.next_attempt_at = OffsetDateTime::now_utc().unix_timestamp()
                            + retry.unwrap_or(
                                (30_i64 * 2_i64.pow(current.attempts.min(7) as u32)).min(3600),
                            );
                        current.last_error = reason.into();
                    }
                    SendResult::Unregistered => {
                        current.state = "cancelled".into();
                        current.last_error = "unregistered".into();
                        if let Some(mut active) = tx.get::<PushDevice>(device.id).await?
                            && active.generation == device.generation
                            && active.token == device.token
                        {
                            active.enabled = false;
                            active.generation += 1;
                            tx.save(&active).await?;
                        }
                    }
                    SendResult::Permanent(reason) => {
                        current.state = "failed".into();
                        current.last_error = reason.into();
                    }
                }
                tx.save(&current).await?;
                Ok(())
            })
        })
        .await?;
    }
    Ok(())
}
