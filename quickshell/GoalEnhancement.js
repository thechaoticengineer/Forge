// Pure terminal-state decision for an asynchronous goal enhancement request.
function goalEnhancementAction(snapshot, requestId, currentText, sentText) {
    if (requestId < 0 || !snapshot || snapshot.request_id !== requestId || snapshot.status === "running")
        return {action: "none"}
    if (snapshot.status === "ready") return {action: currentText === sentText ? "apply" : "offer", text: snapshot.goal}
    if (snapshot.status === "failed") return {action: "error", error: snapshot.error && snapshot.error.trim()
        ? snapshot.error : "Could not enhance the description. Try again."}
    return {action: "none"}
}
