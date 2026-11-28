//! Docstrings of `cmd`, verbatim from CPython 3.12.3 `Lib/cmd.py`.

pub(super) const MODULE_DOC: &str = "A generic class to build line-oriented command interpreters.\n\nInterpreters constructed with this class obey the following conventions:\n\n1. End of file on input is processed as the command 'EOF'.\n2. A command is parsed out of each line by collecting the prefix composed\n   of characters in the identchars member.\n3. A command `foo' is dispatched to a method 'do_foo()'; the do_ method\n   is passed a single argument consisting of the remainder of the line.\n4. Typing an empty line repeats the last command.  (Actually, it calls the\n   method `emptyline', which may be overridden in a subclass.)\n5. There is a predefined `help' method.  Given an argument `topic', it\n   calls the command `help_topic'.  With no arguments, it lists all topics\n   with defined help_ functions, broken into up to three topics; documented\n   commands, miscellaneous help topics, and undocumented commands.\n6. The command '?' is a synonym for `help'.  The command '!' is a synonym\n   for `shell', if a do_shell method exists.\n7. If completion is enabled, completing commands will be done automatically,\n   and completing of commands args is done by calling complete_foo() with\n   arguments text, line, begidx, endidx.  text is string we are matching\n   against, all returned matches must begin with it.  line is the current\n   input line (lstripped), begidx and endidx are the beginning and end\n   indexes of the text being matched, which could be used to provide\n   different completion depending upon which position the argument is in.\n\nThe `default' method may be overridden to intercept commands for which there\nis no do_ method.\n\nThe `completedefault' method may be overridden to intercept completions for\ncommands that have no complete_ method.\n\nThe data member `self.ruler' sets the character used to draw separator lines\nin the help messages.  If empty, no ruler line is drawn.  It defaults to \"=\".\n\nIf the value of `self.intro' is nonempty when the cmdloop method is called,\nit is printed out on interpreter startup.  This value may be overridden\nvia an optional argument to the cmdloop() method.\n\nThe data members `self.doc_header', `self.misc_header', and\n`self.undoc_header' set the headers used for the help function's\nlistings of documented functions, miscellaneous topics, and undocumented\nfunctions respectively.\n";

pub(super) const CLASS_DOC: &str = "A simple framework for writing line-oriented command interpreters.\n\n    These are often useful for test harnesses, administrative tools, and\n    prototypes that will later be wrapped in a more sophisticated interface.\n\n    A Cmd instance or subclass instance is a line-oriented interpreter\n    framework.  There is no good reason to instantiate Cmd itself; rather,\n    it's useful as a superclass of an interpreter class you define yourself\n    in order to inherit Cmd's methods and encapsulate action methods.\n\n    ";

pub(super) const DOC_INIT: &str = "Instantiate a line-oriented interpreter framework.\n\n        The optional argument 'completekey' is the readline name of a\n        completion key; it defaults to the Tab key. If completekey is\n        not None and the readline module is available, command completion\n        is done automatically. The optional arguments stdin and stdout\n        specify alternate input and output file objects; if not specified,\n        sys.stdin and sys.stdout are used.\n\n        ";

pub(super) const DOC_CMDLOOP: &str = "Repeatedly issue a prompt, accept input, parse an initial prefix\n        off the received input, and dispatch to action methods, passing them\n        the remainder of the line as argument.\n\n        ";

pub(super) const DOC_PRECMD: &str = "Hook method executed just before the command line is\n        interpreted, but after the input prompt is generated and issued.\n\n        ";

pub(super) const DOC_POSTCMD: &str =
    "Hook method executed just after a command dispatch is finished.";

pub(super) const DOC_PRELOOP: &str =
    "Hook method executed once when the cmdloop() method is called.";

pub(super) const DOC_POSTLOOP: &str =
    "Hook method executed once when the cmdloop() method is about to\n        return.\n\n        ";

pub(super) const DOC_PARSELINE: &str = "Parse the line into a command name and a string containing\n        the arguments.  Returns a tuple containing (command, args, line).\n        'command' and 'args' may be None if the line couldn't be parsed.\n        ";

pub(super) const DOC_ONECMD: &str = "Interpret the argument as though it had been typed in response\n        to the prompt.\n\n        This may be overridden, but should not normally need to be;\n        see the precmd() and postcmd() methods for useful execution hooks.\n        The return value is a flag indicating whether interpretation of\n        commands by the interpreter should stop.\n\n        ";

pub(super) const DOC_EMPTYLINE: &str = "Called when an empty line is entered in response to the prompt.\n\n        If this method is not overridden, it repeats the last nonempty\n        command entered.\n\n        ";

pub(super) const DOC_DEFAULT: &str = "Called on an input line when the command prefix is not recognized.\n\n        If this method is not overridden, it prints an error message and\n        returns.\n\n        ";

pub(super) const DOC_COMPLETEDEFAULT: &str = "Method called to complete an input line when no command-specific\n        complete_*() method is available.\n\n        By default, it returns an empty list.\n\n        ";

pub(super) const DOC_COMPLETE: &str = "Return the next possible completion for 'text'.\n\n        If a command has not been entered, then complete against command list.\n        Otherwise try to call complete_<command> to get list of completions.\n        ";

pub(super) const DOC_DO_HELP: &str =
    "List available commands with \"help\" or detailed help with \"help cmd\".";

pub(super) const DOC_COLUMNIZE: &str = "Display a list of strings as a compact set of columns.\n\n        Each column is only as wide as necessary.\n        Columns are separated by two spaces (one was not legible enough).\n        ";
