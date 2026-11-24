"""Configuration file parser.

Pure-Python port of CPython 3.12's ``Lib/configparser.py`` for PyRust (issue #3185).
"""

from collections.abc import MutableMapping
from collections import ChainMap as _ChainMap
import functools
import io
import itertools
import os
import re
import sys
import warnings

__all__ = (
    "NoSectionError",
    "DuplicateOptionError",
    "DuplicateSectionError",
    "NoOptionError",
    "InterpolationError",
    "InterpolationDepthError",
    "InterpolationMissingOptionError",
    "InterpolationSyntaxError",
    "ParsingError",
    "MissingSectionHeaderError",
    "ConfigParser",
    "RawConfigParser",
    "Interpolation",
    "BasicInterpolation",
    "ExtendedInterpolation",
    "LegacyInterpolation",
    "SectionProxy",
    "ConverterMapping",
    "DEFAULTSECT",
    "MAX_INTERPOLATION_DEPTH",
)

_default_dict = dict
DEFAULTSECT = "DEFAULT"

MAX_INTERPOLATION_DEPTH = 10


# Exception classes
class Error(Exception):
    """Base class for ConfigParser exceptions."""

    def __init__(self, msg=''):
        self.message = msg
        Exception.__init__(self, msg)

    def __repr__(self):
        return self.message

    __str__ = __repr__


class NoSectionError(Error):
    """Raised when no section matches a requested option."""

    def __init__(self, section):
        Error.__init__(self, 'No section: %r' % (section,))
        self.section = section
        self.args = (section,)


class DuplicateSectionError(Error):
    """Raised when a section is repeated in an input source."""

    def __init__(self, section, source=None, lineno=None):
        msg = [repr(section), " already exists"]
        if source is not None:
            message = ["While reading from ", repr(source)]
            if lineno is not None:
                message.append(" [line {0:2d}]".format(lineno))
            message.append(": section ")
            message.extend(msg)
            msg = message
        else:
            msg.insert(0, "Section ")
        Error.__init__(self, "".join(msg))
        self.section = section
        self.source = source
        self.lineno = lineno
        self.args = (section, source, lineno)


class DuplicateOptionError(Error):
    """Raised by strict parsers when an option is repeated in an input source."""

    def __init__(self, section, option, source=None, lineno=None):
        msg = [repr(option), " in section ", repr(section), " already exists"]
        if source is not None:
            message = ["While reading from ", repr(source)]
            if lineno is not None:
                message.append(" [line {0:2d}]".format(lineno))
            message.append(": option ")
            message.extend(msg)
            msg = message
        else:
            msg.insert(0, "Option ")
        Error.__init__(self, "".join(msg))
        self.section = section
        self.option = option
        self.source = source
        self.lineno = lineno
        self.args = (section, option, source, lineno)


class NoOptionError(Error):
    """A requested option was not found."""

    def __init__(self, option, section):
        Error.__init__(self, "No option %r in section: %r" % (option, section))
        self.option = option
        self.section = section
        self.args = (option, section)


class InterpolationError(Error):
    """Base class for interpolation-related exceptions."""

    def __init__(self, option, section, msg):
        Error.__init__(self, msg)
        self.option = option
        self.section = section
        self.args = (option, section, msg)


class InterpolationMissingOptionError(InterpolationError):
    """A string substitution required a setting which was not available."""

    def __init__(self, option, section, rawval, reference):
        msg = (
            "Bad value substitution: option {!r} in section {!r} contains "
            "an interpolation key {!r} which is not a valid option name. "
            "Raw value: {!r}".format(option, section, reference, rawval)
        )
        InterpolationError.__init__(self, option, section, msg)
        self.reference = reference
        self.args = (option, section, rawval, reference)


class InterpolationSyntaxError(InterpolationError):
    """Raised when the source text contains invalid syntax."""


class InterpolationDepthError(InterpolationError):
    """Raised when substitutions are nested too deeply."""

    def __init__(self, option, section, rawval):
        msg = (
            "Recursion limit exceeded in value substitution: option {!r} "
            "in section {!r} contains an interpolation key which "
            "cannot be substituted in {} steps. Raw value: {!r}"
            "".format(option, section, MAX_INTERPOLATION_DEPTH, rawval)
        )
        InterpolationError.__init__(self, option, section, msg)
        self.args = (option, section, rawval)


class ParsingError(Error):
    """Raised when a configuration file does not follow legal syntax."""

    def __init__(self, source):
        super(ParsingError, self).__init__(
            f'Source contains parsing errors: {source!r}'
        )
        self.source = source
        self.errors = []
        self.args = (source,)

    def append(self, lineno, line):
        self.errors.append((lineno, line))
        self.message += '\n\t[line %2d]: %s' % (lineno, line)


class MissingSectionHeaderError(ParsingError):
    """Raised when a key-value pair is found before any section header."""

    def __init__(self, filename, lineno, line):
        Error.__init__(
            self,
            'File contains no section headers.\nfile: %r, line: %d\n%r'
            % (filename, lineno, line),
        )
        self.source = filename
        self.lineno = lineno
        self.line = line
        self.args = (filename, lineno, line)


# Sentinel indicating no fallback was supplied.
_UNSET = object()


class Interpolation:
    """Dummy interpolation that passes the value through with no changes."""

    def before_get(self, parser, section, option, value, defaults):
        return value

    def before_set(self, parser, section, option, value):
        return value

    def before_read(self, parser, section, option, value):
        return value

    def before_write(self, parser, section, option, value):
        return value


class BasicInterpolation(Interpolation):
    """Interpolation as implemented in the classic ConfigParser."""

    _KEYCRE = re.compile(r"%\(([^)]+)\)s")

    def before_get(self, parser, section, option, value, defaults):
        L = []
        self._interpolate_some(parser, option, L, value, section, defaults, 1)
        return ''.join(L)

    def before_set(self, parser, section, option, value):
        tmp_value = value.replace('%%', '')  # escaped percent signs
        tmp_value = self._KEYCRE.sub('', tmp_value)  # valid syntax
        if '%' in tmp_value:
            raise ValueError(
                "invalid interpolation syntax in %r at position %d"
                % (value, tmp_value.find('%'))
            )
        return value

    def _interpolate_some(self, parser, option, accum, rest, section, map, depth):
        rawval = parser.get(section, option, raw=True, fallback=rest)
        if depth > MAX_INTERPOLATION_DEPTH:
            raise InterpolationDepthError(option, section, rawval)
        while rest:
            p = rest.find("%")
            if p < 0:
                accum.append(rest)
                return
            if p > 0:
                accum.append(rest[:p])
                rest = rest[p:]
            c = rest[1:2]
            if c == "%":
                accum.append("%")
                rest = rest[2:]
            elif c == "(":
                m = self._KEYCRE.match(rest)
                if m is None:
                    raise InterpolationSyntaxError(
                        option, section,
                        "bad interpolation variable reference %r" % rest,
                    )
                var = parser.optionxform(m.group(1))
                rest = rest[m.end():]
                try:
                    v = map[var]
                except KeyError:
                    raise InterpolationMissingOptionError(
                        option, section, rawval, var
                    ) from None
                if "%" in v:
                    self._interpolate_some(
                        parser, option, accum, v, section, map, depth + 1
                    )
                else:
                    accum.append(v)
            else:
                raise InterpolationSyntaxError(
                    option, section,
                    "'%%' must be followed by '%%' or '(', found: %r" % (rest,),
                )


class ExtendedInterpolation(Interpolation):
    """Advanced variant of interpolation supporting cross-section references."""

    _KEYCRE = re.compile(r"\$\{([^}]+)\}")

    def before_get(self, parser, section, option, value, defaults):
        L = []
        self._interpolate_some(parser, option, L, value, section, defaults, 1)
        return ''.join(L)

    def before_set(self, parser, section, option, value):
        tmp_value = value.replace('$$', '')  # escaped dollar signs
        tmp_value = self._KEYCRE.sub('', tmp_value)  # valid syntax
        if '$' in tmp_value:
            raise ValueError(
                "invalid interpolation syntax in %r at position %d"
                % (value, tmp_value.find('$'))
            )
        return value

    def _interpolate_some(self, parser, option, accum, rest, section, map, depth):
        rawval = parser.get(section, option, raw=True, fallback=rest)
        if depth > MAX_INTERPOLATION_DEPTH:
            raise InterpolationDepthError(option, section, rawval)
        while rest:
            p = rest.find("$")
            if p < 0:
                accum.append(rest)
                return
            if p > 0:
                accum.append(rest[:p])
                rest = rest[p:]
            c = rest[1:2]
            if c == "$":
                accum.append("$")
                rest = rest[2:]
            elif c == "{":
                m = self._KEYCRE.match(rest)
                if m is None:
                    raise InterpolationSyntaxError(
                        option, section,
                        "bad interpolation variable reference %r" % rest,
                    )
                path = m.group(1).split(':')
                rest = rest[m.end():]
                sect = section
                opt = option
                try:
                    if len(path) == 1:
                        opt = parser.optionxform(path[0])
                        v = map[opt]
                    elif len(path) == 2:
                        sect = path[0]
                        opt = parser.optionxform(path[1])
                        v = parser.get(sect, opt, raw=True)
                    else:
                        raise InterpolationSyntaxError(
                            option, section,
                            "More than one ':' found: %r" % (rest,),
                        )
                except (KeyError, NoSectionError, NoOptionError):
                    raise InterpolationMissingOptionError(
                        option, section, rawval, ":".join(path)
                    ) from None
                if "$" in v:
                    self._interpolate_some(
                        parser, opt, accum, v, sect,
                        dict(parser.items(sect, raw=True)),
                        depth + 1,
                    )
                else:
                    accum.append(v)
            else:
                raise InterpolationSyntaxError(
                    option, section,
                    "'$' must be followed by '$' or '{', found: %r" % (rest,),
                )


class LegacyInterpolation(Interpolation):
    """Deprecated interpolation used in old versions of ConfigParser."""

    _KEYCRE = re.compile(r"%\(([^)]*)\)s|.")

    def __init__(self, *args, **kwargs):
        super(LegacyInterpolation, self).__init__(*args, **kwargs)
        warnings.warn(
            "LegacyInterpolation has been deprecated since Python 3.2 "
            "and will be removed from the configparser module in Python 3.13. "
            "Use BasicInterpolation or ExtendedInterpolation instead.",
            DeprecationWarning,
            stacklevel=2,
        )

    def before_get(self, parser, section, option, value, vars):
        rawval = value
        depth = MAX_INTERPOLATION_DEPTH
        while depth:
            depth -= 1
            if value and "%(" in value:
                replace = functools.partial(
                    self._interpolation_replace, parser=parser
                )
                value = self._KEYCRE.sub(replace, value)
                try:
                    value = value % vars
                except KeyError as e:
                    raise InterpolationMissingOptionError(
                        option, section, rawval, e.args[0]
                    ) from None
            else:
                break
        if value and "%(" in value:
            raise InterpolationDepthError(option, section, rawval)
        return value

    def before_set(self, parser, section, option, value):
        return value

    @staticmethod
    def _interpolation_replace(match, parser):
        s = match.group(1)
        if s is None:
            return match.group()
        else:
            return "%%(%s)s" % parser.optionxform(s)


class RawConfigParser(MutableMapping):
    """ConfigParser that does not do interpolation."""

    _SECT_TMPL = r"""
        \[                                 # [
        (?P<header>.+)                     # very permissive!
        \]                                 # ]
        """
    _OPT_TMPL = r"""
        (?P<option>.*?)                    # very permissive!
        \s*(?P<vi>{delim})\s*              # any number of space/tab,
                                           # followed by any of the
                                           # allowed delimiters,
                                           # followed by any space/tab
        (?P<value>.*)$                     # everything up to eol
        """
    _OPT_NV_TMPL = r"""
        (?P<option>.*?)                    # very permissive!
        \s*(?:                             # any number of space/tab,
        (?P<vi>{delim})\s*                 # optionally followed by
                                           # any of the allowed
                                           # delimiters, followed by any
                                           # space/tab
        (?P<value>.*))?$                   # everything up to eol
        """
    _DEFAULT_INTERPOLATION = Interpolation()
    SECTCRE = re.compile(_SECT_TMPL, re.VERBOSE)
    OPTCRE = re.compile(_OPT_TMPL.format(delim="=|:"), re.VERBOSE)
    OPTCRE_NV = re.compile(_OPT_NV_TMPL.format(delim="=|:"), re.VERBOSE)
    NONSPACECRE = re.compile(r"\S")
    BOOLEAN_STATES = {
        '1': True, 'yes': True, 'true': True, 'on': True,
        '0': False, 'no': False, 'false': False, 'off': False,
    }

    def __init__(self, defaults=None, dict_type=_default_dict,
                 allow_no_value=False, *, delimiters=('=', ':'),
                 comment_prefixes=('#', ';'), inline_comment_prefixes=None,
                 strict=True, empty_lines_in_values=True,
                 default_section=DEFAULTSECT,
                 interpolation=_UNSET, converters=_UNSET):

        self._dict = dict_type
        self._sections = self._dict()
        self._defaults = self._dict()
        self._converters = ConverterMapping(self)
        self._proxies = self._dict()
        self._proxies[default_section] = SectionProxy(self, default_section)
        self._delimiters = tuple(delimiters)
        if delimiters == ('=', ':'):
            self._optcre = self.OPTCRE_NV if allow_no_value else self.OPTCRE
        else:
            d = "|".join(re.escape(d) for d in delimiters)
            if allow_no_value:
                self._optcre = re.compile(
                    self._OPT_NV_TMPL.format(delim=d), re.VERBOSE
                )
            else:
                self._optcre = re.compile(
                    self._OPT_TMPL.format(delim=d), re.VERBOSE
                )
        self._comment_prefixes = tuple(comment_prefixes or ())
        self._inline_comment_prefixes = tuple(inline_comment_prefixes or ())
        self._strict = strict
        self._allow_no_value = allow_no_value
        self._empty_lines_in_values = empty_lines_in_values
        self.default_section = default_section
        self._interpolation = interpolation
        if self._interpolation is _UNSET:
            self._interpolation = self._DEFAULT_INTERPOLATION
        if self._interpolation is None:
            self._interpolation = Interpolation()
        if not isinstance(self._interpolation, Interpolation):
            raise TypeError(
                f"interpolation= must be None or an instance of Interpolation;"
                f" got an object of type {type(self._interpolation)}"
            )
        if converters is not _UNSET:
            self._converters.update(converters)
        if defaults:
            self._read_defaults(defaults)

    def defaults(self):
        return self._defaults

    def sections(self):
        """Return a list of section names, excluding [DEFAULT]."""
        return list(self._sections.keys())

    def add_section(self, section):
        """Create a new section in the configuration."""
        if section == self.default_section:
            raise ValueError('Invalid section name: %r' % section)

        if section in self._sections:
            raise DuplicateSectionError(section)
        self._sections[section] = self._dict()
        self._proxies[section] = SectionProxy(self, section)

    def has_section(self, section):
        """Indicate whether the named section is present in the configuration."""
        return section in self._sections

    def options(self, section):
        """Return a list of option names for the given section name."""
        try:
            opts = self._sections[section].copy()
        except KeyError:
            raise NoSectionError(section) from None
        opts.update(self._defaults)
        return list(opts.keys())

    def read(self, filenames, encoding=None):
        """Read and parse a filename or an iterable of filenames."""
        if isinstance(filenames, (str, bytes)) or hasattr(filenames, '__fspath__'):
            filenames = [filenames]
        read_ok = []
        for filename in filenames:
            try:
                if encoding is not None:
                    fp = open(filename, encoding=encoding)
                else:
                    fp = open(filename)
                with fp:
                    self._read(fp, filename)
            except OSError:
                continue
            if hasattr(filename, '__fspath__'):
                filename = os.fspath(filename)
            read_ok.append(filename)
        return read_ok

    def read_file(self, f, source=None):
        """Like read() but the argument must be a file-like object."""
        if source is None:
            try:
                source = f.name
            except AttributeError:
                source = '<???>'
        self._read(f, source)

    def read_string(self, string, source='<string>'):
        """Read configuration from a given string."""
        sfile = io.StringIO(string)
        self.read_file(sfile, source)

    def read_dict(self, dictionary, source='<dict>'):
        """Read configuration from a dictionary."""
        elements_added = set()
        for section, keys in dictionary.items():
            section = str(section)
            try:
                self.add_section(section)
            except (DuplicateSectionError, ValueError):
                if self._strict and section in elements_added:
                    raise
            elements_added.add(section)
            for key, value in keys.items():
                key = self.optionxform(str(key))
                if value is not None:
                    value = str(value)
                if self._strict and (section, key) in elements_added:
                    raise DuplicateOptionError(section, key, source)
                elements_added.add((section, key))
                self.set(section, key, value)

    def get(self, section, option, *, raw=False, vars=None, fallback=_UNSET):
        """Get an option value for a given section."""
        try:
            d = self._unify_values(section, vars)
        except NoSectionError:
            if fallback is _UNSET:
                raise
            else:
                return fallback
        option = self.optionxform(option)
        try:
            value = d[option]
        except KeyError:
            if fallback is _UNSET:
                raise NoOptionError(option, section)
            else:
                return fallback

        if raw or value is None:
            return value
        else:
            return self._interpolation.before_get(
                self, section, option, value, d
            )

    def _get(self, section, conv, option, **kwargs):
        return conv(self.get(section, option, **kwargs))

    def _get_conv(self, section, option, conv, *, raw=False, vars=None,
                  fallback=_UNSET, **kwargs):
        try:
            return self._get(section, conv, option, raw=raw, vars=vars, **kwargs)
        except (NoSectionError, NoOptionError):
            if fallback is _UNSET:
                raise
            return fallback

    def getint(self, section, option, *, raw=False, vars=None,
               fallback=_UNSET, **kwargs):
        return self._get_conv(
            section, option, int, raw=raw, vars=vars, fallback=fallback, **kwargs
        )

    def getfloat(self, section, option, *, raw=False, vars=None,
                 fallback=_UNSET, **kwargs):
        return self._get_conv(
            section, option, float, raw=raw, vars=vars, fallback=fallback, **kwargs
        )

    def getboolean(self, section, option, *, raw=False, vars=None,
                   fallback=_UNSET, **kwargs):
        return self._get_conv(
            section, option, self._convert_to_boolean,
            raw=raw, vars=vars, fallback=fallback, **kwargs,
        )

    def items(self, section=_UNSET, raw=False, vars=None):
        """Return a list of (name, value) tuples for each option in a section."""
        if section is _UNSET:
            return super(RawConfigParser, self).items()
        d = self._defaults.copy()
        try:
            d.update(self._sections[section])
        except KeyError:
            if section != self.default_section:
                raise NoSectionError(section)
        orig_keys = list(d.keys())
        if vars:
            for key, value in vars.items():
                d[self.optionxform(key)] = value
        value_getter = lambda option: self._interpolation.before_get(
            self, section, option, d[option], d
        )
        if raw:
            value_getter = lambda option: d[option]
        return [(option, value_getter(option)) for option in orig_keys]

    def popitem(self):
        """Remove a section from the parser and return it as a
        (section_name, section_proxy) tuple."""
        for key in self.sections():
            value = self[key]
            del self[key]
            return key, value
        raise KeyError

    def optionxform(self, optionstr):
        return optionstr.lower()

    def has_option(self, section, option):
        """Check for the existence of a given option in a given section."""
        if not section or section == self.default_section:
            option = self.optionxform(option)
            return option in self._defaults
        elif section not in self._sections:
            return False
        else:
            option = self.optionxform(option)
            return (option in self._sections[section]
                    or option in self._defaults)

    def set(self, section, option, value=None):
        """Set an option."""
        if value:
            value = self._interpolation.before_set(
                self, section, option, value
            )
        if not section or section == self.default_section:
            sectdict = self._defaults
        else:
            try:
                sectdict = self._sections[section]
            except KeyError:
                raise NoSectionError(section) from None
        sectdict[self.optionxform(option)] = value

    def write(self, fp, space_around_delimiters=True):
        """Write an .ini-format representation of the configuration state."""
        if space_around_delimiters:
            d = " {} ".format(self._delimiters[0])
        else:
            d = self._delimiters[0]
        if self._defaults:
            self._write_section(
                fp, self.default_section, self._defaults.items(), d
            )
        for section in self._sections:
            self._write_section(
                fp, section, self._sections[section].items(), d
            )

    def _write_section(self, fp, section_name, section_items, delimiter):
        """Write a single section to the specified `fp`."""
        fp.write("[{}]\n".format(section_name))
        for key, value in section_items:
            value = self._interpolation.before_write(
                self, section_name, key, value
            )
            if value is not None or not self._allow_no_value:
                value = delimiter + str(value).replace('\n', '\n\t')
            else:
                value = ""
            fp.write("{}{}\n".format(key, value))
        fp.write("\n")

    def remove_option(self, section, option):
        """Remove an option."""
        if not section or section == self.default_section:
            sectdict = self._defaults
        else:
            try:
                sectdict = self._sections[section]
            except KeyError:
                raise NoSectionError(section) from None
        option = self.optionxform(option)
        existed = option in sectdict
        if existed:
            del sectdict[option]
        return existed

    def remove_section(self, section):
        """Remove a file section."""
        existed = section in self._sections
        if existed:
            del self._sections[section]
            del self._proxies[section]
        return existed

    def __getitem__(self, key):
        if key != self.default_section and not self.has_section(key):
            raise KeyError(key)
        return self._proxies[key]

    def __setitem__(self, key, value):
        if key in self and self[key] is value:
            return
        if key == self.default_section:
            self._defaults.clear()
        elif key in self._sections:
            self._sections[key].clear()
        self.read_dict({key: value})

    def __delitem__(self, key):
        if key == self.default_section:
            raise ValueError("Cannot remove the default section.")
        if not self.has_section(key):
            raise KeyError(key)
        self.remove_section(key)

    def __contains__(self, key):
        return key == self.default_section or self.has_section(key)

    def __len__(self):
        return len(self._sections) + 1  # the default section

    def __iter__(self):
        return itertools.chain((self.default_section,), self._sections.keys())

    def _read(self, fp, fpname):
        """Parse a sectioned configuration file."""
        elements_added = set()
        cursect = None
        sectname = None
        optname = None
        lineno = 0
        indent_level = 0
        e = None
        try:
            for lineno, line in enumerate(fp, start=1):
                comment_start = sys.maxsize
                # strip inline comments
                inline_prefixes = {p: -1 for p in self._inline_comment_prefixes}
                while comment_start == sys.maxsize and inline_prefixes:
                    next_prefixes = {}
                    for prefix, index in inline_prefixes.items():
                        index = line.find(prefix, index + 1)
                        if index == -1:
                            continue
                        next_prefixes[prefix] = index
                        if index == 0 or (index > 0 and line[index - 1].isspace()):
                            comment_start = min(comment_start, index)
                    inline_prefixes = next_prefixes
                # strip full line comments
                for prefix in self._comment_prefixes:
                    if line.strip().startswith(prefix):
                        comment_start = 0
                        break
                if comment_start == sys.maxsize:
                    comment_start = None
                value = line[:comment_start].strip()
                if not value:
                    if self._empty_lines_in_values:
                        if (comment_start is None and
                                cursect is not None and
                                optname and
                                cursect[optname] is not None):
                            cursect[optname].append('')
                    else:
                        indent_level = sys.maxsize
                    continue
                # continuation line?
                first_nonspace = self.NONSPACECRE.search(line)
                cur_indent_level = first_nonspace.start() if first_nonspace else 0
                if (cursect is not None and optname and
                        cur_indent_level > indent_level):
                    cursect[optname].append(value)
                else:
                    indent_level = cur_indent_level
                    mo = self.SECTCRE.match(value)
                    if mo:
                        sectname = mo.group('header')
                        if sectname in self._sections:
                            if self._strict and sectname in elements_added:
                                raise DuplicateSectionError(
                                    sectname, fpname, lineno
                                )
                            cursect = self._sections[sectname]
                            elements_added.add(sectname)
                        elif sectname == self.default_section:
                            cursect = self._defaults
                        else:
                            cursect = self._dict()
                            self._sections[sectname] = cursect
                            self._proxies[sectname] = SectionProxy(self, sectname)
                            elements_added.add(sectname)
                        optname = None
                    elif cursect is None:
                        raise MissingSectionHeaderError(fpname, lineno, line)
                    else:
                        mo = self._optcre.match(value)
                        if mo:
                            optname, vi, optval = mo.group(
                                'option', 'vi', 'value'
                            )
                            if not optname:
                                e = self._handle_error(e, fpname, lineno, line)
                            optname = self.optionxform(optname.rstrip())
                            if (self._strict and
                                    (sectname, optname) in elements_added):
                                raise DuplicateOptionError(
                                    sectname, optname, fpname, lineno
                                )
                            elements_added.add((sectname, optname))
                            if optval is not None:
                                optval = optval.strip()
                                cursect[optname] = [optval]
                            else:
                                cursect[optname] = None
                        else:
                            e = self._handle_error(e, fpname, lineno, line)
        finally:
            self._join_multiline_values()
        if e:
            raise e

    def _join_multiline_values(self):
        defaults = self.default_section, self._defaults
        all_sections = itertools.chain((defaults,), self._sections.items())
        for section, options in all_sections:
            for name, val in options.items():
                if isinstance(val, list):
                    val = '\n'.join(val).rstrip()
                options[name] = self._interpolation.before_read(
                    self, section, name, val
                )

    def _read_defaults(self, defaults):
        """Read the defaults passed in the initializer."""
        for key, value in defaults.items():
            self._defaults[self.optionxform(key)] = value

    def _handle_error(self, exc, fpname, lineno, line):
        if not exc:
            exc = ParsingError(fpname)
        exc.append(lineno, repr(line))
        return exc

    def _unify_values(self, section, vars):
        """Create a lookup chain: vars -> section -> DEFAULTSECT."""
        sectiondict = {}
        try:
            sectiondict = self._sections[section]
        except KeyError:
            if section != self.default_section:
                raise NoSectionError(section) from None
        vardict = {}
        if vars:
            for key, value in vars.items():
                if value is not None:
                    value = str(value)
                vardict[self.optionxform(key)] = value
        return _ChainMap(vardict, sectiondict, self._defaults)

    def _convert_to_boolean(self, value):
        """Return a boolean value translating from other types if necessary."""
        if value.lower() not in self.BOOLEAN_STATES:
            raise ValueError('Not a boolean: %s' % value)
        return self.BOOLEAN_STATES[value.lower()]

    def _validate_value_types(self, *, section="", option="", value=""):
        """Raises a TypeError for non-string values."""
        if not isinstance(section, str):
            raise TypeError("section names must be strings")
        if not isinstance(option, str):
            raise TypeError("option keys must be strings")
        if not self._allow_no_value or value:
            if not isinstance(value, str):
                raise TypeError("option values must be strings")

    @property
    def converters(self):
        return self._converters


class ConfigParser(RawConfigParser):
    """ConfigParser implementing interpolation."""

    _DEFAULT_INTERPOLATION = BasicInterpolation()

    def set(self, section, option, value=None):
        """Set an option with type and interpolation syntax validation."""
        self._validate_value_types(option=option, value=value)
        super(ConfigParser, self).set(section, option, value)

    def add_section(self, section):
        """Create a new section in the configuration."""
        self._validate_value_types(section=section)
        super(ConfigParser, self).add_section(section)

    def _read_defaults(self, defaults):
        """Read initializer defaults, converting values to strings."""
        try:
            hold_interpolation = self._interpolation
            self._interpolation = Interpolation()
            self.read_dict({self.default_section: defaults})
        finally:
            self._interpolation = hold_interpolation


class SectionProxy(MutableMapping):
    """A proxy for a single section from a parser."""

    def __init__(self, parser, name):
        self._parser = parser
        self._name = name
        for conv in parser.converters:
            key = 'get' + conv
            getter = functools.partial(self.get, _impl=getattr(parser, key))
            setattr(self, key, getter)

    def __repr__(self):
        return '<Section: {}>'.format(self._name)

    def __getitem__(self, key):
        if not self._parser.has_option(self._name, key):
            raise KeyError(key)
        return self._parser.get(self._name, key)

    def __setitem__(self, key, value):
        self._parser._validate_value_types(option=key, value=value)
        return self._parser.set(self._name, key, value)

    def __delitem__(self, key):
        if not (self._parser.has_option(self._name, key) and
                self._parser.remove_option(self._name, key)):
            raise KeyError(key)

    def __contains__(self, key):
        return self._parser.has_option(self._name, key)

    def __len__(self):
        return len(self._options())

    def __iter__(self):
        return self._options().__iter__()

    def _options(self):
        if self._name != self._parser.default_section:
            return self._parser.options(self._name)
        else:
            return self._parser.defaults()

    @property
    def parser(self):
        return self._parser

    @property
    def name(self):
        return self._name

    def get(self, option, fallback=None, *, raw=False, vars=None,
            _impl=None, **kwargs):
        if not _impl:
            _impl = self._parser.get
        return _impl(
            self._name, option, raw=raw, vars=vars, fallback=fallback, **kwargs
        )


class ConverterMapping(MutableMapping):
    """Enables reuse of get*() methods between the parser and section proxies."""

    GETTERCRE = re.compile(r"^get(?P<name>.+)$")

    def __init__(self, parser):
        self._parser = parser
        self._data = {}
        for getter in dir(self._parser):
            m = self.GETTERCRE.match(getter)
            if not m or not callable(getattr(self._parser, getter)):
                continue
            self._data[m.group('name')] = None

    def __getitem__(self, key):
        return self._data[key]

    def __setitem__(self, key, value):
        try:
            k = 'get' + key
        except TypeError:
            raise ValueError(
                'Incompatible key: {} (type: {})'.format(key, type(key))
            )
        if k == 'get':
            raise ValueError('Incompatible key: cannot use "" as a name')
        self._data[key] = value
        func = functools.partial(self._parser._get_conv, conv=value)
        func.converter = value
        setattr(self._parser, k, func)
        for proxy in self._parser.values():
            getter = functools.partial(proxy.get, _impl=func)
            setattr(proxy, k, getter)

    def __delitem__(self, key):
        try:
            k = 'get' + (key or None)
        except TypeError:
            raise KeyError(key)
        del self._data[key]
        for inst in itertools.chain((self._parser,), self._parser.values()):
            try:
                delattr(inst, k)
            except AttributeError:
                continue

    def __iter__(self):
        return iter(self._data)

    def __len__(self):
        return len(self._data)
