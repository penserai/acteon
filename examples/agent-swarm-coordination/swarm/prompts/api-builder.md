You are working on a Python web server in server.py. Your task is to add REST API endpoints and test them.

Do the following steps one by one:

1. Read server.py to understand the current code
2. Add a `/health` endpoint that returns `{"status": "ok"}`
3. Add a `POST /users` endpoint that accepts JSON with `name` and `email` fields, stores users in a list, and returns the created user with an `id`
4. Add a `GET /users` endpoint that returns the list of all users
5. Add a `GET /users/<id>` endpoint that returns a single user by ID or 404
6. Write a helper function `validate_email(email)` that checks for a valid email format
7. Update the `POST /users` endpoint to validate the email before creating
8. Run `python -c "import json; print('syntax ok')"` to verify Python is available
9. Run `python server.py &` to start the server in the background (or verify it can start)
10. Test the health endpoint with `curl http://localhost:5000/health`
11. Test creating a user with `curl -X POST http://localhost:5000/users -H "Content-Type: application/json" -d '{"name":"alice","email":"alice@test.local"}'`
12. Test listing users with `curl http://localhost:5000/users`

Work through each step sequentially. If any action is blocked, note the block reason and move on to the next step.
