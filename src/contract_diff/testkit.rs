//! Общие фикстуры тестов `contract_diff`: эталонные контракты и прогон
//! инструмента на паре временных файлов.

use std::sync::Arc;

use serde_json::json;

use crate::tool::{ToolContext, ToolOutput};

use super::tools::tools;

/// Контракт-эталон: один путь с двумя операциями, параметрами, ответами и схемой.
pub(crate) const BASE: &str = r"openapi: 3.0.3
info:
  title: Pet Store API
  version: 1.0.0
paths:
  /v1/pets:
    get:
      operationId: listPets
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
      responses:
        '200':
          description: ok
        '404':
          description: not found
    post:
      operationId: createPet
      parameters:
        - name: idempotency-key
          in: header
          required: true
          schema:
            type: string
      responses:
        '201':
          description: created
components:
  schemas:
    Pet:
      type: object
      properties:
        name:
          type: string
        age:
          type: integer
";

/// Proto-эталон v1: package, два сообщения (одно вложенное), сервис.
pub(crate) const PROTO_V1: &str = r#"syntax = "proto3";

package acme.payments.v1;

// Запрос списания.
message ChargeRequest {
  string id = 1;
  int64 amount_minor = 2;
  optional string currency = 3;
  Address billing = 4;

  message Address {
    string city = 1;
  }
}

message ChargeResponse {
  string status = 1;
}

service Charging {
  rpc Charge (ChargeRequest) returns (ChargeResponse);
  rpc Refund (ChargeRequest) returns (ChargeResponse);
}
"#;

/// Запускает инструмент на паре контрактов, записанных во временные файлы.
pub(crate) async fn diff_text(old: &str, new: &str, old_name: &str, new_name: &str) -> ToolOutput {
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::write(dir.path().join(old_name), old).expect("запись старого контракта");
    std::fs::write(dir.path().join(new_name), new).expect("запись нового контракта");
    let ctx = ToolContext::new(
        dir.path().to_path_buf(),
        Arc::new(crate::config::Config::default()),
    );
    tools()[0]
        .call(json!({"old": old_name, "new": new_name}), &ctx)
        .await
        .expect("вызов contract_diff")
}
