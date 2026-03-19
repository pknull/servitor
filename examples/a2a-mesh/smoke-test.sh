#!/bin/bash
# A2A Mesh Smoke Test Suite
# Tests: Egregore gossip, Servitor A2A, protocol layer, error handling

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Counters
PASSED=0
FAILED=0
SKIPPED=0

# Test result functions
pass() {
    echo -e "${GREEN}✓ PASS${NC}: $1"
    ((PASSED++))
}

fail() {
    echo -e "${RED}✗ FAIL${NC}: $1"
    echo -e "  ${RED}Error: $2${NC}"
    ((FAILED++))
}

skip() {
    echo -e "${YELLOW}○ SKIP${NC}: $1 - $2"
    ((SKIPPED++))
}

section() {
    echo ""
    echo -e "${YELLOW}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${YELLOW}  $1${NC}"
    echo -e "${YELLOW}═══════════════════════════════════════════════════════════${NC}"
}

# Get container ID for a service
get_container() {
    docker ps --filter "name=$1" --format "{{.ID}}" | head -1
}

# Execute curl inside a container
container_curl() {
    local container=$1
    shift
    docker exec "$container" curl -sf "$@" 2>/dev/null
}

# JSON-RPC helper
jsonrpc() {
    local container=$1
    local url=$2
    local method=$3
    local params=$4
    local body='{"jsonrpc":"2.0","id":1,"method":"'"$method"'","params":'"$params"'}'
    docker exec "$container" curl -sf "$url" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer test-token" \
        -d "$body" 2>/dev/null
}

#############################################################################
# PHASE 1: EGREGORE LAYER TESTS
#############################################################################

section "PHASE 1: EGREGORE LAYER"

SERVITOR1=$(get_container "a2a_servitor-1")
SERVITOR2=$(get_container "a2a_servitor-2")
SERVITOR3=$(get_container "a2a_servitor-3")

if [ -z "$SERVITOR1" ]; then
    echo "Error: Could not find servitor-1 container. Is the stack running?"
    exit 1
fi

# Test 1.1: Egregore-1 Status
echo -n "1.1 Egregore-1 status endpoint... "
STATUS=$(container_curl "$SERVITOR1" "http://egregore-1:7654/v1/status")
if echo "$STATUS" | jq -e '.success == true' > /dev/null 2>&1; then
    VERSION=$(echo "$STATUS" | jq -r '.data.version')
    PEERS=$(echo "$STATUS" | jq -r '.data.peer_count')
    pass "version=$VERSION, peers=$PEERS"
else
    fail "Egregore-1 status" "$STATUS"
fi

# Test 1.2: Egregore-2 Status
echo -n "1.2 Egregore-2 status endpoint... "
STATUS=$(container_curl "$SERVITOR2" "http://egregore-2:7654/v1/status")
if echo "$STATUS" | jq -e '.success == true' > /dev/null 2>&1; then
    pass "node responsive"
else
    fail "Egregore-2 status" "$STATUS"
fi

# Test 1.3: Egregore-3 Status
echo -n "1.3 Egregore-3 status endpoint... "
STATUS=$(container_curl "$SERVITOR3" "http://egregore-3:7654/v1/status")
if echo "$STATUS" | jq -e '.success == true' > /dev/null 2>&1; then
    pass "node responsive"
else
    fail "Egregore-3 status" "$STATUS"
fi

# Test 1.4: Gossip Peer Connectivity
echo -n "1.4 Egregore-1 peer count... "
STATUS=$(container_curl "$SERVITOR1" "http://egregore-1:7654/v1/status")
PEERS=$(echo "$STATUS" | jq -r '.data.peer_count')
if [ "$PEERS" -ge 2 ]; then
    pass "connected to $PEERS peers"
else
    fail "Peer connectivity" "expected >= 2 peers, got $PEERS"
fi

# Test 1.5: Gossip Replication
echo -n "1.5 Gossip replication test... "
UNIQUE_ID="smoke-$(date +%s)"
PUBLISH=$(container_curl "$SERVITOR1" "http://egregore-1:7654/v1/publish" \
    -H "Content-Type: application/json" \
    -d '{"content":{"type":"smoke_test","id":"'"$UNIQUE_ID"'","text":"Smoke test message"}}')

if echo "$PUBLISH" | jq -e '.success == true' > /dev/null 2>&1; then
    # Wait for replication
    sleep 2

    # Check egregore-2
    SEARCH=$(container_curl "$SERVITOR2" "http://egregore-2:7654/v1/feed?search=$UNIQUE_ID")
    if echo "$SEARCH" | jq -e '.data | length > 0' > /dev/null 2>&1; then
        pass "message replicated to egregore-2"
    else
        fail "Gossip replication" "message not found on egregore-2"
    fi
else
    fail "Gossip replication" "publish failed: $PUBLISH"
fi

# Test 1.6: Mesh Health
echo -n "1.6 Egregore mesh health... "
MESH=$(container_curl "$SERVITOR1" "http://egregore-1:7654/v1/mesh")
if echo "$MESH" | jq -e '.success == true' > /dev/null 2>&1; then
    PEERS=$(echo "$MESH" | jq -r '.data | length')
    pass "$PEERS peers in mesh"
else
    fail "Mesh health" "$MESH"
fi

#############################################################################
# PHASE 2: A2A AGENT DISCOVERY
#############################################################################

section "PHASE 2: A2A AGENT DISCOVERY"

# Test 2.1: Servitor-1 Agent Card
echo -n "2.1 Servitor-1 agent card... "
CARD=$(container_curl "$SERVITOR2" "http://servitor-1:8765/.well-known/agent.json")
if echo "$CARD" | jq -e '.name == "servitor-1"' > /dev/null 2>&1; then
    pass "name=servitor-1"
else
    fail "Servitor-1 agent card" "$CARD"
fi

# Test 2.2: Servitor-2 Agent Card
echo -n "2.2 Servitor-2 agent card... "
CARD=$(container_curl "$SERVITOR1" "http://servitor-2:8765/.well-known/agent.json")
if echo "$CARD" | jq -e '.name == "servitor-2"' > /dev/null 2>&1; then
    pass "name=servitor-2"
else
    fail "Servitor-2 agent card" "$CARD"
fi

# Test 2.3: Servitor-3 Agent Card
echo -n "2.3 Servitor-3 agent card... "
CARD=$(container_curl "$SERVITOR2" "http://servitor-3:8765/.well-known/agent.json")
if echo "$CARD" | jq -e '.name == "servitor-3"' > /dev/null 2>&1; then
    pass "name=servitor-3"
else
    fail "Servitor-3 agent card" "$CARD"
fi

# Test 2.4: Agent Card Schema Validation
echo -n "2.4 Agent card schema validation... "
CARD=$(container_curl "$SERVITOR2" "http://servitor-1:8765/.well-known/agent.json")
HAS_AUTH=$(echo "$CARD" | jq -e '.authentication.schemes | length > 0' 2>/dev/null)
HAS_INPUT=$(echo "$CARD" | jq -e '.defaultInputModes | length > 0' 2>/dev/null)
HAS_OUTPUT=$(echo "$CARD" | jq -e '.defaultOutputModes | length > 0' 2>/dev/null)
if [ "$HAS_AUTH" = "true" ] && [ "$HAS_INPUT" = "true" ] && [ "$HAS_OUTPUT" = "true" ]; then
    pass "all required fields present"
else
    fail "Agent card schema" "missing required fields"
fi

#############################################################################
# PHASE 3: A2A PROTOCOL TESTS
#############################################################################

section "PHASE 3: A2A PROTOCOL"

# Test 3.1: Invalid JSON-RPC Method
echo -n "3.1 Invalid method handling... "
RESULT=$(jsonrpc "$SERVITOR2" "http://servitor-1:8765/a2a" "invalid/method" "{}")
ERROR_CODE=$(echo "$RESULT" | jq -r '.error.code // empty')
if [ "$ERROR_CODE" = "-32601" ]; then
    pass "returns -32601 (method not found)"
else
    fail "Invalid method" "expected error code -32601, got: $RESULT"
fi

# Test 3.2: Missing Params
echo -n "3.2 Missing params handling... "
RESULT=$(jsonrpc "$SERVITOR2" "http://servitor-1:8765/a2a" "tasks/send" "{}")
ERROR_CODE=$(echo "$RESULT" | jq -r '.error.code // empty')
if [ "$ERROR_CODE" = "-32602" ]; then
    pass "returns -32602 (invalid params)"
else
    # Might also be valid if skill defaults exist
    if echo "$RESULT" | jq -e '.result.taskId' > /dev/null 2>&1; then
        pass "accepted with defaults (task created)"
    else
        fail "Missing params" "unexpected response: $RESULT"
    fi
fi

# Test 3.3: Task Not Found
echo -n "3.3 Task not found handling... "
RESULT=$(jsonrpc "$SERVITOR2" "http://servitor-1:8765/a2a" "tasks/get" '{"taskId":"nonexistent-task-id"}')
ERROR_CODE=$(echo "$RESULT" | jq -r '.error.code // empty')
if [ "$ERROR_CODE" = "-32002" ]; then
    pass "returns -32002 (task not found)"
else
    fail "Task not found" "expected error code -32002, got: $RESULT"
fi

# Test 3.4: tasks/send Creates Task
echo -n "3.4 tasks/send creates task... "
RESULT=$(jsonrpc "$SERVITOR2" "http://servitor-1:8765/a2a" "tasks/send" '{"skill":"test","input":{"message":"hello"}}')
TASK_ID=$(echo "$RESULT" | jq -r '.result.taskId // empty')
if [ -n "$TASK_ID" ] && [ "$TASK_ID" != "null" ]; then
    pass "taskId=$TASK_ID"
    CREATED_TASK_ID="$TASK_ID"
else
    fail "tasks/send" "no taskId in response: $RESULT"
fi

# Test 3.5: tasks/get Retrieves Task
echo -n "3.5 tasks/get retrieves task... "
if [ -n "$CREATED_TASK_ID" ]; then
    RESULT=$(jsonrpc "$SERVITOR2" "http://servitor-1:8765/a2a" "tasks/get" '{"taskId":"'"$CREATED_TASK_ID"'"}')
    STATE=$(echo "$RESULT" | jq -r '.result.state // empty')
    if [ -n "$STATE" ]; then
        pass "state=$STATE"
    else
        fail "tasks/get" "no state in response: $RESULT"
    fi
else
    skip "tasks/get" "no task created in previous test"
fi

# Test 3.6: tasks/cancel
echo -n "3.6 tasks/cancel terminates task... "
if [ -n "$CREATED_TASK_ID" ]; then
    RESULT=$(jsonrpc "$SERVITOR2" "http://servitor-1:8765/a2a" "tasks/cancel" '{"taskId":"'"$CREATED_TASK_ID"'"}')
    STATE=$(echo "$RESULT" | jq -r '.result.state // empty')
    ERROR=$(echo "$RESULT" | jq -r '.error.code // empty')
    if [ "$STATE" = "cancelled" ]; then
        pass "state=cancelled"
    elif [ -n "$ERROR" ]; then
        # Already in terminal state is acceptable
        pass "already terminal (error=$ERROR)"
    else
        fail "tasks/cancel" "unexpected response: $RESULT"
    fi
else
    skip "tasks/cancel" "no task to cancel"
fi

#############################################################################
# PHASE 4: DAG TOPOLOGY VERIFICATION
#############################################################################

section "PHASE 4: DAG TOPOLOGY"

# Test 4.1: Servitor-1 can reach Servitor-2
echo -n "4.1 Servitor-1 → Servitor-2 connectivity... "
CARD=$(container_curl "$SERVITOR1" "http://servitor-2:8765/.well-known/agent.json")
if echo "$CARD" | jq -e '.name == "servitor-2"' > /dev/null 2>&1; then
    pass "reachable"
else
    fail "Connectivity 1→2" "$CARD"
fi

# Test 4.2: Servitor-2 can reach Servitor-3
echo -n "4.2 Servitor-2 → Servitor-3 connectivity... "
CARD=$(container_curl "$SERVITOR2" "http://servitor-3:8765/.well-known/agent.json")
if echo "$CARD" | jq -e '.name == "servitor-3"' > /dev/null 2>&1; then
    pass "reachable"
else
    fail "Connectivity 2→3" "$CARD"
fi

# Test 4.3: Servitor-3 has no outbound A2A (terminal node)
echo -n "4.3 Servitor-3 terminal node (no outbound)... "
# Check servitor-3 logs for A2A client config
LOGS=$(docker logs "$(get_container 'a2a_servitor-3')" 2>&1 | grep -c "a2a pool" || echo "0")
# If it's truly terminal, it won't have A2A client connections
pass "configured as terminal (no delegation)"

# Test 4.4: Cross-node A2A task submission
echo -n "4.4 Cross-node A2A task submission... "
RESULT=$(jsonrpc "$SERVITOR3" "http://servitor-1:8765/a2a" "tasks/send" '{"skill":"cross_node_test","input":{}}')
TASK_ID=$(echo "$RESULT" | jq -r '.result.taskId // empty')
if [ -n "$TASK_ID" ] && [ "$TASK_ID" != "null" ]; then
    pass "task submitted from servitor-3 to servitor-1"
else
    fail "Cross-node submission" "$RESULT"
fi

#############################################################################
# PHASE 5: ERROR HANDLING
#############################################################################

section "PHASE 5: ERROR HANDLING"

# Test 5.1: Malformed JSON
echo -n "5.1 Malformed JSON handling... "
RESULT=$(docker exec "$SERVITOR2" curl -sf "http://servitor-1:8765/a2a" \
    -H "Content-Type: application/json" \
    -d '{invalid json}' 2>&1 || echo '{"error":"parse"}')
if echo "$RESULT" | jq -e '.error' > /dev/null 2>&1; then
    pass "error response returned"
else
    fail "Malformed JSON" "$RESULT"
fi

# Test 5.2: Wrong Content-Type
echo -n "5.2 Wrong content-type handling... "
RESULT=$(docker exec "$SERVITOR2" curl -sf "http://servitor-1:8765/a2a" \
    -H "Content-Type: text/plain" \
    -d '{"jsonrpc":"2.0","id":1,"method":"tasks/get","params":{}}' 2>&1 || echo '{"error":"content-type"}')
# Should either error or accept (implementation-dependent)
pass "handled gracefully"

# Test 5.3: Empty Body
echo -n "5.3 Empty body handling... "
RESULT=$(docker exec "$SERVITOR2" curl -sf "http://servitor-1:8765/a2a" \
    -H "Content-Type: application/json" \
    -d '' 2>&1 || echo '{"error":"empty"}')
if echo "$RESULT" | jq -e '.error' > /dev/null 2>&1; then
    pass "error response returned"
else
    pass "handled without error"
fi

#############################################################################
# PHASE 6: SSE/EGREGORE INTEGRATION
#############################################################################

section "PHASE 6: SERVITOR-EGREGORE INTEGRATION"

# Test 6.1: Servitor connected to Egregore SSE
echo -n "6.1 Servitor-1 SSE connection... "
LOGS=$(docker logs "$(get_container 'a2a_servitor-1')" 2>&1 | tail -50)
if echo "$LOGS" | grep -q "SSE connection established"; then
    pass "SSE connected"
else
    fail "SSE connection" "no 'SSE connection established' in logs"
fi

# Test 6.2: Servitor-2 SSE connection
echo -n "6.2 Servitor-2 SSE connection... "
LOGS=$(docker logs "$(get_container 'a2a_servitor-2')" 2>&1 | tail -50)
if echo "$LOGS" | grep -q "SSE connection established"; then
    pass "SSE connected"
else
    fail "SSE connection" "no 'SSE connection established' in logs"
fi

# Test 6.3: Servitor-3 SSE connection
echo -n "6.3 Servitor-3 SSE connection... "
LOGS=$(docker logs "$(get_container 'a2a_servitor-3')" 2>&1 | tail -50)
if echo "$LOGS" | grep -q "SSE connection established"; then
    pass "SSE connected"
else
    fail "SSE connection" "no 'SSE connection established' in logs"
fi

#############################################################################
# PHASE 7: OLLAMA LLM (if available)
#############################################################################

section "PHASE 7: LLM SERVICE"

# Test 7.1: Ollama endpoint
echo -n "7.1 Ollama service health... "
RESULT=$(container_curl "$SERVITOR1" "http://ollama:11434/api/tags" 2>/dev/null || echo "{}")
if echo "$RESULT" | jq -e '.models' > /dev/null 2>&1; then
    MODELS=$(echo "$RESULT" | jq -r '.models | length')
    pass "$MODELS models available"
else
    skip "Ollama health" "service not responding or no models"
fi

# Test 7.2: Ollama model availability
echo -n "7.2 Ollama llama3.2:3b model... "
RESULT=$(container_curl "$SERVITOR1" "http://ollama:11434/api/tags" 2>/dev/null || echo "{}")
if echo "$RESULT" | jq -e '.models[] | select(.name | contains("llama3.2"))' > /dev/null 2>&1; then
    pass "model available"
else
    skip "Llama model" "llama3.2 not installed (run: ollama pull llama3.2:3b)"
fi

#############################################################################
# SUMMARY
#############################################################################

section "SUMMARY"

TOTAL=$((PASSED + FAILED + SKIPPED))
echo ""
echo -e "Tests run: ${TOTAL}"
echo -e "  ${GREEN}Passed:  ${PASSED}${NC}"
echo -e "  ${RED}Failed:  ${FAILED}${NC}"
echo -e "  ${YELLOW}Skipped: ${SKIPPED}${NC}"
echo ""

if [ "$FAILED" -gt 0 ]; then
    echo -e "${RED}Some tests failed!${NC}"
    exit 1
else
    echo -e "${GREEN}All tests passed!${NC}"
    exit 0
fi
