import SwiftUI
import UIKit

/// Keeps a transcript at its end while it grows, unless the person has scrolled away.
///
/// It works on the `UIScrollView` behind SwiftUI's `ScrollView` rather than through SwiftUI's
/// anchors and scroll geometry. Those don't say which coordinates their offsets and insets are
/// in, and an initial anchor on a lazy stack opened long transcripts at the top. UIKit's
/// numbers are exact, and pinning when the content size changes happens in the same layout
/// pass, so a growing transcript never draws a frame short of its end.
@MainActor @Observable
final class TranscriptScroller {
    /// Whether to offer a way back to the end: not following, and far from it. Closer, the
    /// end is a short flick away and the button is clutter.
    private(set) var isAway = false
    /// Whether the transcript has settled at its end after opening, until which it's hidden,
    /// so it doesn't show its top and then scroll.
    private(set) var isReady = false

    @ObservationIgnored private var follow = Follow()
    @ObservationIgnored private weak var scrollView: UIScrollView?
    @ObservationIgnored private var observations: [NSKeyValueObservation] = []
    @ObservationIgnored private var lastOffset: CGFloat = 0

    /// How far from the end still counts as there, so coming to rest a little short of it
    /// still follows.
    private static let endSlack: CGFloat = 40
    /// The way back shows scrolling up past `awayFrom`, and goes scrolling down inside
    /// `backWithin`. Only the direction the person is going can change it, because a lazy
    /// stack's distance to the end jumps as it trades estimated row heights for measured ones,
    /// which flicked the button off and on while scrolling one way.
    private static let awayFrom: CGFloat = 500
    private static let backWithin: CGFloat = 400
    /// Layout passes to wait for a lazy stack to measure the rows at its end after opening.
    private static let settleAttempts = 6

    /// Found from inside the scroll view's content, once it's in a window.
    func attach(_ scrollView: UIScrollView) {
        guard scrollView !== self.scrollView else { return }
        self.scrollView = scrollView
        observations = [
            scrollView.observe(\.contentSize, options: [.new]) { [weak self] _, _ in
                MainActor.assumeIsolated { self?.contentChanged() }
            },
            scrollView.observe(\.contentOffset, options: [.new]) { [weak self] _, _ in
                MainActor.assumeIsolated { self?.scrolled() }
            },
        ]
        settle(attempt: 0)
    }

    /// Shows the transcript even if it never settled, so a scroll view that can't be found
    /// costs following rather than the whole transcript.
    func revealEventually() async {
        try? await Task.sleep(for: .milliseconds(600))
        isReady = true
    }

    func dragStarted() {
        follow.dragStarted()
        refresh()
    }

    func cameToRest() {
        guard let scrollView else { return }
        if follow.cameToRest(atEnd: Self.distanceFromEnd(scrollView) <= Self.endSlack) { pin() }
        refresh()
    }

    func unfolded(last: Bool) {
        follow.unfolded(last: last)
        refresh()
    }

    func jumpToEnd() {
        follow.resume()
        guard let scrollView else { return }
        scrollView.setContentOffset(CGPoint(x: scrollView.contentOffset.x, y: Self.end(of: scrollView)), animated: true)
        refresh()
    }

    /// The container or its insets changed, such as the keyboard or the message box growing.
    func containerChanged() {
        contentChanged()
    }

    /// Only the person moves the way back in and out. Content arriving while they read
    /// pushes the end further away, and brought the button back after they'd scrolled it off.
    private func scrolled() {
        guard let scrollView else { return }
        let offset = scrollView.contentOffset.y
        defer { lastOffset = offset }
        guard follow.byHand, !follow.isOn else { return }
        let distance = Self.distanceFromEnd(scrollView)
        if offset < lastOffset && distance > Self.awayFrom && !isAway {
            isAway = true
        } else if offset > lastOffset && distance < Self.backWithin && isAway {
            isAway = false
        }
    }

    private func contentChanged() {
        guard follow.isOn else { return }
        if !follow.byHand && scrollView?.isTracking == false { pin() }
        refresh()
    }

    /// Pins to the end, then checks again after each layout, because moving down a lazy stack
    /// measures rows it had only estimated, which moves the end.
    private func settle(attempt: Int) {
        pin()
        DispatchQueue.main.async { [weak self] in
            guard let self, let scrollView else { return }
            // Past the end counts too: found before its first layout, the end was a guess.
            let off = abs(Self.distanceFromEnd(scrollView)) > 1
            if off && follow.isOn && attempt < Self.settleAttempts {
                settle(attempt: attempt + 1)
            } else {
                isReady = true
            }
        }
    }

    private func pin() {
        guard let scrollView else { return }
        let end = Self.end(of: scrollView)
        if abs(scrollView.contentOffset.y - end) > 0.5 {
            scrollView.setContentOffset(CGPoint(x: scrollView.contentOffset.x, y: end), animated: false)
        }
    }

    private func refresh() {
        if follow.isOn && isAway { isAway = false }
    }

    /// The offset that shows the end, or the top when everything fits.
    private static func end(of scrollView: UIScrollView) -> CGFloat {
        let insets = scrollView.adjustedContentInset
        return max(-insets.top, scrollView.contentSize.height + insets.bottom - scrollView.bounds.height)
    }

    private static func distanceFromEnd(_ scrollView: UIScrollView) -> CGFloat {
        end(of: scrollView) - scrollView.contentOffset.y
    }
}

/// Hands the scroll view it's inside to `found`, once it's in a window.
struct ScrollViewFinder: UIViewRepresentable {
    let found: (UIScrollView) -> Void

    func makeUIView(context: Context) -> FinderView {
        FinderView(found: found)
    }

    func updateUIView(_ view: FinderView, context: Context) {}

    final class FinderView: UIView {
        let found: (UIScrollView) -> Void

        init(found: @escaping (UIScrollView) -> Void) {
            self.found = found
            super.init(frame: .zero)
            isUserInteractionEnabled = false
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) {
            fatalError("not used from a storyboard")
        }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            guard window != nil else { return }
            var view = superview
            while let current = view {
                if let scrollView = current as? UIScrollView {
                    found(scrollView)
                    return
                }
                view = current.superview
            }
        }
    }
}
