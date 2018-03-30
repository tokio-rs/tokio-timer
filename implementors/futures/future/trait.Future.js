(function() {var implementors = {};
implementors["tokio_timer"] = [{text:"impl <a class=\"trait\" href=\"https://docs.rs/futures/0.1/futures/future/trait.Future.html\" title=\"trait futures::future::Future\">Future</a> for <a class=\"struct\" href=\"tokio_timer/struct.Sleep.html\" title=\"struct tokio_timer::Sleep\">Sleep</a>",synthetic:false,types:["tokio_timer::timer::Sleep"]},{text:"impl&lt;F, E&gt; <a class=\"trait\" href=\"https://docs.rs/futures/0.1/futures/future/trait.Future.html\" title=\"trait futures::future::Future\">Future</a> for <a class=\"struct\" href=\"tokio_timer/struct.Timeout.html\" title=\"struct tokio_timer::Timeout\">Timeout</a>&lt;F&gt; <span class=\"where fmt-newline\">where<br>&nbsp;&nbsp;&nbsp;&nbsp;F: <a class=\"trait\" href=\"https://docs.rs/futures/0.1/futures/future/trait.Future.html\" title=\"trait futures::future::Future\">Future</a>&lt;Error = E&gt;,<br>&nbsp;&nbsp;&nbsp;&nbsp;E: <a class=\"trait\" href=\"https://doc.rust-lang.org/nightly/core/convert/trait.From.html\" title=\"trait core::convert::From\">From</a>&lt;<a class=\"enum\" href=\"tokio_timer/enum.TimeoutError.html\" title=\"enum tokio_timer::TimeoutError\">TimeoutError</a>&lt;F&gt;&gt;,&nbsp;</span>",synthetic:false,types:["tokio_timer::timer::Timeout"]},];

            if (window.register_implementors) {
                window.register_implementors(implementors);
            } else {
                window.pending_implementors = implementors;
            }
        
})()
