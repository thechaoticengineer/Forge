// Pure plan-editor transformations. Callers own focus, selection and request state.
function cloneStages(plan) {
    return JSON.parse(JSON.stringify(plan.stages))
}

function modelConstraint(stage, key, value) {
    const constraint = Object.assign({}, stage.model_constraint || {})
    if (value.trim()) constraint[key] = value.trim()
    else delete constraint[key]
    return Object.keys(constraint).length ? constraint : null
}

function editableNeighbor(stages, index, direction) {
    for (let target = index + direction; target >= 0 && target < stages.length; target += direction) {
        if (stages[target].status !== "committed") return target
    }
    return -1
}

function moveStages(stages, index, target) {
    const moved = stages.slice(), stage = moved[index]
    moved[index] = moved[target]
    moved[target] = stage
    return moved
}

function deleteStage(stages, index) {
    const edited = stages.slice()
    edited.splice(index, 1)
    return edited
}

function addStage(stages) {
    return stages.concat([{title: "", instructions: "", acceptance: "", commit: ""}])
}

function payload(goal, stages) {
    return {goal: goal, stages: stages.map(function(stage) {
        const content = {title: stage.title, instructions: stage.instructions,
            acceptance: stage.acceptance, commit: stage.commit}
        content.model_constraint = stage.model_constraint || null
        if (stage.depends_on !== undefined) content.depends_on = stage.depends_on
        if (stage.id !== undefined) content.id = stage.id
        return content
    })}
}
