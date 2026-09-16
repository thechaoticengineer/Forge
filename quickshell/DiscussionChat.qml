pragma ComponentBehavior: Bound

import QtQuick
import "Discussion.js" as Discussion

// Interface: the transcript, in-flight activity and the message queued for
// sending enter as properties; copying, leaving focus, revealing an
// off-screen control and reporting user activity leave as signals. Panel.qml
// owns the HTTP calls, request correlation and transcript persistence.
Item {
  id: chat

  required property var entries
  required property bool pending
  required property string sentMessage
  required property string error
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property string fontFamily
  required property real fontSize

  readonly property alias followTail: list.followTail
  readonly property real contentPadding: 8

  signal copyRequested(string original)
  signal leaveRequested()
  signal focusRevealed(var control)
  signal inspecting()

  implicitHeight: Math.max(list.contentHeight, rows.count === 0 ? placeholder.implicitHeight : 0) + chat.contentPadding * 2

  ListModel { id: rows }

  function refresh() {
    Discussion.reconcileChat(rows, Discussion.chatMessages(chat.entries, chat.pending, chat.sentMessage, chat.error))
    if (rows.count === 0) {
      list.followTail = true
      list.readingY = 0
    }
  }
  onEntriesChanged: chat.refresh()
  onPendingChanged: chat.refresh()
  onSentMessageChanged: chat.refresh()
  onErrorChanged: chat.refresh()
  Component.onCompleted: chat.refresh()

  Flickable {
    id: list
    objectName: "discussionChatList"
    anchors.fill: parent
    anchors.margins: chat.contentPadding
    clip: true
    boundsBehavior: Flickable.StopAtBounds
    contentWidth: width
    contentHeight: column.height

    property bool followTail: true
    property real readingY: 0
    function scrollToTail() { if (followTail && !moving) contentY = Math.max(0, contentHeight - height) }
    function restoreReadingPosition() {
      if (!followTail && !moving) contentY = Math.max(0, Math.min(readingY, Math.max(0, contentHeight - height)))
    }
    onContentYChanged: if (moving) { followTail = atYEnd; readingY = contentY }
    onMovementEnded: { followTail = atYEnd; readingY = contentY }
    onContentHeightChanged: Qt.callLater(function() { restoreReadingPosition(); scrollToTail() })
    onHeightChanged: Qt.callLater(function() { restoreReadingPosition(); scrollToTail() })
    onVisibleChanged: if (visible) Qt.callLater(scrollToTail)

    Column {
      id: column
      width: list.width
      spacing: 6
      Repeater {
        model: rows
        delegate: DiscussionMessage {
          id: delegate
          required property var model
          width: column.width
          kind: delegate.model.kind
          outgoing: delegate.model.outgoing
          author: delegate.model.author
          status: delegate.model.status
          messageText: delegate.model.text
          foreground: chat.foreground
          mutedForeground: chat.mutedForeground
          background: chat.background
          surface: chat.surface
          accent: chat.accent
          urgent: chat.urgent
          fontFamily: chat.fontFamily
          fontSize: chat.fontSize
          onCopyRequested: original => chat.copyRequested(original)
          onLeaveRequested: chat.leaveRequested()
          onFocusRevealed: control => chat.focusRevealed(control)
          onInspecting: {
            list.followTail = false
            list.readingY = list.contentY
            chat.inspecting()
          }
        }
      }
    }
  }

  Text {
    id: placeholder
    anchors.centerIn: parent
    visible: rows.count === 0
    text: "No messages yet. Ask Forge about this project below."
    textFormat: Text.PlainText
    wrapMode: Text.Wrap
    width: chat.width - chat.contentPadding * 2
    horizontalAlignment: Text.AlignHCenter
    color: chat.mutedForeground
    font.family: chat.fontFamily
    font.pixelSize: chat.fontSize
  }
}
