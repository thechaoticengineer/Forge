pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons

// Interface: the discussion transcript, activity, the message being sent and
// gating booleans enter as properties and render as a chat; sending a
// message, planning from the discussion, clearing, expanding and focus
// handoff leave as signals. Panel.qml owns HTTP calls, request correlation
// and outer-scroll coordination.
Column {
  id: view

  required property var entries
  required property bool pending
  required property string sentMessage
  required property string error
  required property bool canSend
  required property bool canPlan
  required property bool canClear
  required property bool expanded
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property string fontFamily
  required property real fontSize11
  required property real fontSize12

  property alias input: messageInput

  signal sendRequested(string message)
  signal planRequested
  signal clearRequested
  signal expansionRequested(bool expanded)
  signal leaveRequested
  signal helpRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  function send() {
    const text = messageInput.text
    if (!view.canSend || text.trim() === "") return
    view.sendRequested(text)
  }

  width: parent ? parent.width : 0
  spacing: Style.space(6)

  Row {
    width: parent.width
    spacing: Style.space(8)
    ViewButton {
      id: toggleButton
      label: (view.expanded ? "▾" : "▸") + " Discuss before planning"
        + (view.entries.length > 0 ? " (" + view.entries.length + ")" : "")
      onClicked: view.expansionRequested(!view.expanded)
    }
  }

  Rectangle {
    visible: view.expanded
    width: parent.width
    height: discussionChat.height
    color: view.surface
    radius: 4
    DiscussionChat {
      id: discussionChat
      objectName: "discussionTranscript"
      width: parent.width
      entries: view.entries
      pending: view.pending
      sentMessage: view.sentMessage
      error: view.error
      maximumHeight: Style.space(320)
      foreground: view.foreground
      mutedForeground: view.mutedForeground
      background: view.background
      surface: view.surface
      accent: view.accent
      urgent: view.urgent
      fontFamily: view.fontFamily
      fontSize: view.fontSize12
      onCopyRequested: original => Quickshell.clipboardText = original
      onLeaveRequested: view.leaveRequested()
      onFocusRevealed: control => view.detailRevealed(control)
      onInspecting: view.detailInspected(discussionChat)
    }
  }

  Text {
    width: parent.width
    visible: !view.expanded && text !== ""
    text: view.pending ? "Forge is replying…" : view.error
    textFormat: Text.PlainText
    color: view.error !== "" ? view.urgent : view.mutedForeground
    wrapMode: Text.Wrap
    font.family: view.fontFamily
    font.pixelSize: view.fontSize11
  }

  Rectangle {
    width: parent.width
    height: Math.min(Math.max(Style.space(44),
      messageInput.contentHeight + Style.space(16)), Style.space(110))
    color: view.surface
    radius: 4
    border.width: 1
    border.color: messageInput.activeFocus ? view.accent : Qt.darker(view.foreground, 3)
    Flickable {
      id: messageFlick
      anchors.fill: parent
      anchors.margins: Style.space(6)
      clip: true
      contentWidth: messageInput.width
      contentHeight: messageInput.height
      flickableDirection: Flickable.VerticalFlick
      boundsBehavior: Flickable.StopAtBounds

      function ensureCursorVisible() {
        const cursor = messageInput.cursorRectangle
        if (contentY > cursor.y)
          contentY = cursor.y
        else if (contentY + height < cursor.y + cursor.height)
          contentY = cursor.y + cursor.height - height
        contentY = Math.max(0, Math.min(contentY, contentHeight - height))
      }

      onHeightChanged: Qt.callLater(ensureCursorVisible)
      onContentHeightChanged: Qt.callLater(ensureCursorVisible)

      TextEdit {
        id: messageInput
        width: messageFlick.width
        height: Math.max(contentHeight, messageFlick.height)
        wrapMode: TextEdit.Wrap
        color: view.foreground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize12
        onCursorRectangleChanged: messageFlick.ensureCursorVisible()
        Keys.onPressed: event => {
          if (event.key === Qt.Key_F1) {
            view.helpRequested()
            view.leaveRequested()
            event.accepted = true
          } else if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                     && event.modifiers === Qt.NoModifier) {
            view.send()
            event.accepted = true
          }
        }
        Keys.onEscapePressed: event => {
          view.leaveRequested()
          event.accepted = true
        }
        Text {
          visible: messageInput.text === "" && !messageInput.activeFocus
          text: "Message Forge about this project… (Enter to send, Shift+Enter for newline)"
          color: view.mutedForeground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize12
        }
      }
    }
  }

  Row {
    width: parent.width
    spacing: Style.space(8)
    ViewButton {
      id: sendButton
      label: view.pending ? "Sending…" : "Send"
      enabled: view.canSend && messageInput.text.trim() !== ""
      onClicked: view.send()
    }
    ViewButton {
      id: planFromDiscussionButton
      label: "Create plan from discussion"
      enabled: view.canPlan
      onClicked: view.planRequested()
    }
    ViewButton {
      id: clearDiscussionButton
      label: "Clear discussion"
      enabled: view.canClear
      onClicked: view.clearRequested()
    }
  }

  component ViewButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
  }
}
