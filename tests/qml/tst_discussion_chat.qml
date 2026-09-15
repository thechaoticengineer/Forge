import QtQuick
import QtTest
import "../../quickshell"

Item {
    id: fixture
    width: 600
    height: 560
    property string copied: ""
    property bool leftRequested: false
    property int parentKeys: 0
    Keys.onPressed: event => { parentKeys++; event.accepted = true }

    DiscussionChat {
        id: chat
        width: 480
        maximumHeight: 150
        entries: []
        pending: false
        sentMessage: ""
        error: ""
        foreground: "#dddddd"
        mutedForeground: "#aaaaaa"
        background: "#202020"
        surface: "#282828"
        accent: "#6699ff"
        urgent: "#ff6666"
        fontFamily: "monospace"
        fontSize: 12
        onCopyRequested: original => fixture.copied = original
        onLeaveRequested: fixture.leftRequested = true
    }

    TestCase {
        name: "DiscussionChat"
        when: windowShown

        function walk(item, name) {
            const found = []
            function rec(it) {
                const kids = it.children || []
                for (let i = 0; i < kids.length; ++i) {
                    if (kids[i].objectName === name) found.push(kids[i])
                    rec(kids[i])
                }
            }
            rec(item)
            return found
        }
        function labels(item) {
            const found = []
            function rec(it) {
                const kids = it.children || []
                for (let i = 0; i < kids.length; ++i) {
                    if (kids[i].text !== undefined && kids[i].objectName !== "discussionMessageText")
                        found.push(kids[i].text)
                    rec(kids[i])
                }
            }
            rec(item)
            return found
        }
        function entry(role, text, unix) { return { role: role, text: text, unix: unix } }

        function init() {
            chat.entries = []
            chat.pending = false
            chat.sentMessage = ""
            chat.error = ""
            chat.maximumHeight = 150
            chat.width = 480
            fixture.copied = ""
            fixture.leftRequested = false
            fixture.parentKeys = 0
            wait(30)
        }

        function test_bubbles_render_in_order_with_distinct_alignment_and_fill() {
            chat.entries = [entry("user", "Hi there", 1), entry("assistant", "Hello, how can I help?", 2)]
            wait(30)
            const bubbles = walk(chat, "discussionBubble")
            compare(bubbles.length, 2)
            const bodies = walk(chat, "discussionMessageText")
            compare(bodies.map(b => b.text), ["Hi there", "Hello, how can I help?"])

            const userBubble = bubbles[0]
            const forgeBubble = bubbles[1]
            verify(Math.abs(userBubble.x + userBubble.width - userBubble.parent.width) < 1,
                "the user's bubble hugs the right edge")
            compare(forgeBubble.x, 0, "Forge's bubble hugs the left edge")
            verify(userBubble.color !== forgeBubble.color, "outgoing and incoming bubbles use different fills")
            compare(userBubble.color, Qt.alpha(chat.accent, 0.18))
            compare(forgeBubble.color, chat.surface)
        }

        function test_pending_reply_and_failed_reply_render_last() {
            chat.entries = [entry("user", "Earlier question", 1), entry("assistant", "Earlier answer", 2)]
            chat.pending = true
            chat.sentMessage = "What about tests?"
            wait(30)
            let bodies = walk(chat, "discussionMessageText")
            compare(bodies.length, 4)
            compare(bodies[2].text, "What about tests?")
            compare(bodies[3].text, "Forge is replying…")
            const bubbles = walk(chat, "discussionBubble")
            verify(labels(bubbles[2]).some(t => t.indexOf("Sending…") !== -1),
                "the sending bubble shows its status")

            chat.pending = false
            chat.sentMessage = ""
            chat.error = "Could not reach the engine."
            wait(30)
            bodies = walk(chat, "discussionMessageText")
            compare(bodies.length, 3)
            const last = bodies[bodies.length - 1]
            compare(last.text, "Could not reach the engine.")
            compare(last.color, chat.urgent)
        }

        function test_follow_tail_scrolls_and_reading_position_is_kept() {
            const list = findChild(chat, "discussionChatList")
            const many = []
            for (let i = 0; i < 20; ++i)
                many.push(entry(i % 2 === 0 ? "user" : "assistant", "message number " + i, i))
            chat.entries = many
            wait(30)
            verify(chat.followTail, "a fresh conversation follows the tail")
            verify(list.atYEnd, "the view starts scrolled to the newest message")

            list.contentY = 0
            list.movementEnded()
            wait(30)
            compare(chat.followTail, false, "scrolling away from the tail stops following it")
            const readingPosition = list.contentY

            chat.entries = many.concat([entry("assistant", "one more", 99)])
            wait(30)
            compare(Math.round(list.contentY), Math.round(readingPosition),
                "a reader scrolled away from the tail keeps their position")

            list.followTail = true
            list.scrollToTail()
            wait(30)
            verify(list.atYEnd, "returning to the tail resumes following new messages")
        }

        function test_identical_reassignment_preserves_focus_and_selection() {
            const base = [entry("user", "First message", 1), entry("assistant", "Second message", 2)]
            chat.entries = base
            wait(30)
            const bodies = walk(chat, "discussionMessageText")
            bodies[0].select(0, 5)
            const selection = bodies[0].selectedText
            const copyButtons = walk(chat, "discussionCopy")
            copyButtons[1].forceActiveFocus()
            verify(copyButtons[1].activeFocus)

            chat.entries = base.slice()
            wait(30)
            compare(walk(chat, "discussionCopy")[1], copyButtons[1])
            verify(copyButtons[1].activeFocus, "focus survives an identical reassignment")
            compare(walk(chat, "discussionMessageText")[0], bodies[0])
            compare(bodies[0].selectedText, selection, "selection survives an identical reassignment")
        }

        function test_copy_button_emits_copy_requested() {
            chat.entries = [entry("user", "Copy this text", 1)]
            wait(30)
            const copyButtons = walk(chat, "discussionCopy")
            compare(copyButtons.length, 1)
            mouseClick(copyButtons[0])
            compare(fixture.copied, "Copy this text")
        }

        function test_tab_traversal_and_escape_and_key_isolation() {
            chat.entries = [entry("user", "Alpha", 1), entry("assistant", "Beta", 2)]
            wait(30)
            const bodies = walk(chat, "discussionMessageText")
            const copyButtons = walk(chat, "discussionCopy")

            // Document order is [copy0, body0, copy1, body1]: each message's
            // compact copy button sits above its body text in the header.
            copyButtons[0].forceActiveFocus()
            verify(copyButtons[0].activeFocus)
            const before = fixture.parentKeys
            keyClick(Qt.Key_R)
            compare(fixture.parentKeys, before, "an ordinary letter key does not reach a parent Keys handler")

            keyClick(Qt.Key_Tab)
            wait(10)
            verify(bodies[0].activeFocus, "Tab moves from a message's copy button to its own body text")

            keyClick(Qt.Key_Tab)
            wait(10)
            verify(copyButtons[1].activeFocus, "Tab moves into the next message's controls")

            keyClick(Qt.Key_Backtab)
            wait(10)
            verify(bodies[0].activeFocus, "Backtab moves focus toward an earlier message control")

            keyClick(Qt.Key_Escape)
            verify(fixture.leftRequested, "Escape leaves the focused message")
        }
    }
}
