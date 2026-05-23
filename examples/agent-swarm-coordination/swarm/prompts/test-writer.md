You are working on a Python web server in server.py. Your task is to write tests for it.

Do the following steps one by one:

1. Read server.py to understand the current code and endpoints
2. Run `pip install pytest` to install the test framework
3. Create a file `test_server.py` with pytest tests covering:
   - Test that the server module can be imported
   - Test any utility functions defined in server.py
   - Test that route handlers exist
4. Add a test that verifies the server starts without errors
5. Add a test for request/response handling if the framework supports a test client
6. Run `pytest test_server.py -v` to execute the tests
7. If any tests fail, read the error output and fix the test code
8. Run `pytest test_server.py -v` again to confirm all tests pass
9. Add a test for edge cases (empty input, missing fields)
10. Run the full test suite one final time: `pytest test_server.py -v --tb=short`

Work through each step sequentially. If any action is blocked (e.g., pip install requires approval), note the block reason and move on to the next step.
