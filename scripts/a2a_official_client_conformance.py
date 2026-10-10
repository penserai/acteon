#!/usr/bin/env python3
"""Exercise Acteon with the pinned official A2A Python SDK."""

from __future__ import annotations

import argparse
import asyncio

import httpx
from a2a.client import ClientConfig, create_client
from a2a.helpers import new_text_message
from a2a.types import (
    AuthenticationInfo,
    CancelTaskRequest,
    DeleteTaskPushNotificationConfigRequest,
    GetTaskPushNotificationConfigRequest,
    GetTaskRequest,
    ListTaskPushNotificationConfigsRequest,
    ListTasksRequest,
    Role,
    SendMessageConfiguration,
    SendMessageRequest,
    SubscribeToTaskRequest,
    TaskPushNotificationConfig,
    TaskState,
)


async def run(base_url: str, token: str) -> None:
    headers = {"Authorization": f"Bearer {token}"}
    async with httpx.AsyncClient(headers=headers, timeout=15.0) as http:
        # Exercise the official JSON-RPC streaming transport selected from the
        # discovered Agent Card. The first frame is the durable Task snapshot;
        # this harness closes after that frame because no worker is attached to
        # advance the synthetic task autonomously.
        config = ClientConfig(streaming=True, polling=False, httpx_client=http)
        client = await create_client(base_url, client_config=config)
        try:
            request = SendMessageRequest(
                message=new_text_message(
                    "inspect the governed city",
                    role=Role.ROLE_USER,
                ),
                configuration=SendMessageConfiguration(
                    return_immediately=True,
                    task_push_notification_config=TaskPushNotificationConfig(
                        id="inline-push-config",
                        url="https://example.com/acteon-a2a-inline",
                    ),
                ),
            )
            response_stream = client.send_message(request)
            first = await anext(response_stream)
            await response_stream.aclose()
            assert first.HasField("task")
            submitted = first.task
            assert submitted.status.state == TaskState.TASK_STATE_SUBMITTED
            assert submitted.history[0].role == Role.ROLE_USER

            subscription = client.subscribe(SubscribeToTaskRequest(id=submitted.id))
            subscribed = await anext(subscription)
            await subscription.aclose()
            assert subscribed.HasField("task")
            assert subscribed.task.id == submitted.id
            assert subscribed.task.status.state == TaskState.TASK_STATE_SUBMITTED

            observed = await client.get_task(GetTaskRequest(id=submitted.id))
            assert observed.id == submitted.id
            assert observed.status.state == TaskState.TASK_STATE_SUBMITTED

            listed = await client.list_tasks(ListTasksRequest(page_size=10))
            assert listed.page_size == 10
            assert listed.total_size >= 1
            assert submitted.id in {task.id for task in listed.tasks}

            inline_push = await client.get_task_push_notification_config(
                GetTaskPushNotificationConfigRequest(
                    task_id=submitted.id,
                    id="inline-push-config",
                )
            )
            assert inline_push.task_id == submitted.id
            await client.delete_task_push_notification_config(
                DeleteTaskPushNotificationConfigRequest(
                    task_id=submitted.id,
                    id=inline_push.id,
                )
            )

            created_push = await client.create_task_push_notification_config(
                TaskPushNotificationConfig(
                    task_id=submitted.id,
                    url="https://example.com/acteon-a2a-conformance",
                    token="push-token",
                    authentication=AuthenticationInfo(
                        scheme="Bearer",
                        credentials="push-secret",
                    ),
                )
            )
            assert created_push.id
            assert created_push.task_id == submitted.id
            observed_push = await client.get_task_push_notification_config(
                GetTaskPushNotificationConfigRequest(
                    task_id=submitted.id,
                    id=created_push.id,
                )
            )
            assert observed_push.url == created_push.url
            push_page = await client.list_task_push_notification_configs(
                ListTaskPushNotificationConfigsRequest(
                    task_id=submitted.id,
                    page_size=10,
                )
            )
            assert created_push.id in {config.id for config in push_page.configs}
            await client.delete_task_push_notification_config(
                DeleteTaskPushNotificationConfigRequest(
                    task_id=submitted.id,
                    id=created_push.id,
                )
            )
            await client.delete_task_push_notification_config(
                DeleteTaskPushNotificationConfigRequest(
                    task_id=submitted.id,
                    id=created_push.id,
                )
            )

            canceled = await client.cancel_task(CancelTaskRequest(id=submitted.id))
            assert canceled.id == submitted.id
            assert canceled.status.state == TaskState.TASK_STATE_CANCELED

            replayed = await client.get_task(GetTaskRequest(id=submitted.id))
            assert replayed.status.state == TaskState.TASK_STATE_CANCELED
            print(
                "official a2a-sdk conformance: "
                f"task={submitted.id} stream/subscribe/inline-push/get/list/"
                "push-crud/cancel/get passed"
            )
        finally:
            await client.close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", required=True)
    args = parser.parse_args()
    asyncio.run(run(args.base_url, args.token))


if __name__ == "__main__":
    main()
