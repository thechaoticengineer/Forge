pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "DetailText.js" as DetailText

// Interface: one chat row enters as properties (kind, author, status, text
// plus palette and fonts); copying, leaving focus, revealing an off-screen
// control and reporting user activity leave as signals. The conversation and
// Panel.qml own layout, scrolling and the clipboard.
FocusScope {
  id: message

  required property string kind
  required property bool outgoing
  required property string author
  required property string status
  required property string messageText
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property string fontFamily
  required property real fontSize

  signal copyRequested(string original)
  signal leaveRequested()
  signal focusRevealed(var control)
  signal inspecting()

  readonly property bool isPending: message.kind === "pending"
  readonly property bool isError: message.kind === "error"
  readonly property real maxBubbleWidth: Math.max(120, message.width * 0.8)
  readonly property real bubblePadding: 8

  implicitHeight: bubble.height
  onActiveFocusChanged: if (activeFocus) message.inspecting()

  // A pointer press puts focus where the user already clicked, and moving the
  // view there would interrupt a drag selection. Every other reason, chiefly
  // Tab and Backtab, may have moved focus outside the visible area.
  function revealFocus(control, reason) {
    if (reason !== Qt.MouseFocusReason && reason !== Qt.PopupFocusReason)
      message.focusRevealed(control)
  }

  // Child controls process selection/copy first. Unhandled keys cannot bubble
  // into the panel's action shortcuts; Tab still uses normal focus traversal.
  Keys.priority: Keys.AfterItem
  Keys.onPressed: event => {
    if (event.key === Qt.Key_Escape) {
      message.leaveRequested()
      event.accepted = true
    } else if (event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) {
      event.accepted = true
    }
  }

  component CopyButton: Button {
    id: control
    implicitWidth: buttonLabel.implicitWidth + 16
    implicitHeight: Math.max(22, buttonLabel.implicitHeight + 8)
    padding: 4
    font.family: message.fontFamily
    font.pixelSize: Math.max(9, message.fontSize - 2)
    contentItem: Text {
      id: buttonLabel
      text: control.text
      textFormat: Text.PlainText
      font: control.font
      color: message.foreground
      opacity: control.enabled ? 1 : 0.5
      horizontalAlignment: Text.AlignHCenter
      verticalAlignment: Text.AlignVCenter
      elide: Text.ElideRight
    }
    background: Rectangle {
      radius: 4
      color: control.down ? Qt.alpha(message.foreground, 0.12)
        : control.hovered ? Qt.alpha(message.foreground, 0.06) : "transparent"
      border.width: 1
      border.color: Qt.alpha(message.foreground, control.visualFocus ? 0.8 : 0.18)
    }
  }

  Rectangle {
    id: bubble
    objectName: "discussionBubble"
    x: message.outgoing ? message.width - width : 0
    width: content.width + message.bubblePadding * 2
    height: content.height + message.bubblePadding * 2
    radius: 8
    clip: true
    color: message.outgoing ? Qt.alpha(message.accent, 0.18) : message.surface
    border.width: 1
    border.color: message.isError ? message.urgent
      : message.outgoing ? message.accent : Qt.alpha(message.foreground, 0.18)

    Column {
      id: content
      x: message.bubblePadding
      y: message.bubblePadding
      spacing: 4
      width: Math.max(header.implicitWidth, bodyText.contentWidth)

      Row {
        id: header
        spacing: 6
        layoutDirection: message.outgoing ? Qt.RightToLeft : Qt.LeftToRight
        x: message.outgoing ? content.width - width : 0
        Text {
          textFormat: Text.PlainText
          text: message.author + (message.status !== "" ? " · " + message.status : "")
          color: message.isError ? message.urgent : message.mutedForeground
          font.family: message.fontFamily
          font.pixelSize: Math.max(9, message.fontSize - 2)
          font.italic: message.isPending
        }
        CopyButton {
          id: copyButton
          objectName: "discussionCopy"
          visible: !message.isPending
          text: "Copy"
          focusPolicy: Qt.StrongFocus
          activeFocusOnTab: true
          Accessible.name: "Copy message"
          onActiveFocusChanged: if (activeFocus) message.revealFocus(copyButton, focusReason)
          onClicked: {
            message.inspecting()
            message.copyRequested(DetailText.copySource(message.messageText))
          }
        }
      }

      TextArea {
        id: bodyText
        objectName: "discussionMessageText"
        width: message.maxBubbleWidth - message.bubblePadding * 2
        readOnly: true
        selectByMouse: true
        persistentSelection: true
        activeFocusOnTab: true
        textFormat: TextEdit.PlainText
        text: message.messageText
        wrapMode: DetailText.wrapsWords(message.messageText) ? TextEdit.Wrap : TextEdit.WrapAnywhere
        color: message.isError ? message.urgent : message.isPending ? message.mutedForeground : message.foreground
        font.family: message.fontFamily
        font.pixelSize: message.fontSize
        font.italic: message.isPending
        padding: 0
        background: null
        onActiveFocusChanged: if (activeFocus) message.revealFocus(bodyText, focusReason)
        onSelectedTextChanged: if (selectedText !== "") message.inspecting()
      }
    }
  }
}
