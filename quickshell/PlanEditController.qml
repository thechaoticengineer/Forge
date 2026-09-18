import QtQuick
import "PlanEdit.js" as PlanEdit

// Interface: the hand-editing actions of the Plan view (e). The edit state stays
// on the panel, which PlanEditorView and the guards read; Panel.qml passes host
// (the edit state, plan, selection, guards and act()), its key handler and the
// editor, and maps the editor's signals to these functions. Non-visual.
Item {
  id: controller

  property var host: null
  property var keyTarget: null
  property var editor: null

  function beginPlanEdit() {
    if (!host.guards.editPlan) return
    // Editing happens in the Plan view, wherever it was started from.
    host.currentTab = "plan"
    host.editStages = PlanEdit.cloneStages(host.plan)
    host.editGoal = host.plan.goal || ""
    host.editProject = host.lastProject
    host.editSession++
    host.editingPlan = true
    host.localError = ""
    keyTarget.forceActiveFocus()
    keyTarget.selectStage(host.selectedStageIndex < 0 ? 0 : host.selectedStageIndex)
  }

  function cancelPlanEdit() {
    if (host.editingPlan) keyTarget.forceActiveFocus()
    host.editingPlan = false
    host.editPending = false
    host.editFocusedField = null
    host.editStages = []
    host.editGoal = ""
    host.editProject = ""
    host.editSession++
    host.selectedStageIndex = Math.min(host.selectedStageIndex, host.displayedStages.length - 1)
  }

  function changeStageField(index, field, value) {
    const stages = host.editStages
    if (!host.editingPlan || host.editPending || !stages[index]
        || stages[index].status === "committed") return
    stages[index][field] = value
    host.editRevision++
  }

  function changeModelConstraint(index, key, value) {
    const stage = host.editStages[index]
    changeStageField(index, "model_constraint", PlanEdit.modelConstraint(stage, key, value))
  }

  function moveEditStage(index, direction) {
    if (host.editPending || host.editStages[index].status === "committed") return
    const target = PlanEdit.editableNeighbor(host.editStages, index, direction)
    if (target < 0) return
    keyTarget.forceActiveFocus()
    host.editStages = PlanEdit.moveStages(host.editStages, index, target)
    keyTarget.selectStage(target)
  }

  function deleteEditStage(index) {
    if (host.editPending || host.editStages[index].status === "committed") return
    keyTarget.forceActiveFocus()
    const stages = PlanEdit.deleteStage(host.editStages, index)
    host.editStages = stages
    host.selectedStageIndex = -1
    keyTarget.selectStage(Math.min(index, stages.length - 1))
  }

  function addEditStage() {
    if (host.editPending) return
    keyTarget.forceActiveFocus()
    host.editStages = PlanEdit.addStage(host.editStages)
    keyTarget.selectStage(host.editStages.length - 1)
  }

  function savePlanEdit() {
    if (!editor.saveButton.enabled) return
    keyTarget.forceActiveFocus()
    const session = host.editSession
    const content = PlanEdit.payload(host.editGoal, host.editStages)
    host.editPending = true
    host.act("/api/plan/edit", { project: host.editProject, plan: content }, function(resp) {
      if (session !== controller.host.editSession) return
      controller.host.editPending = false
      if (resp && resp.ok) controller.cancelPlanEdit()
      else if (!resp || !resp.error)
        controller.host.localError = "Could not save plan. Check the engine connection and try again."
    })
  }
}
