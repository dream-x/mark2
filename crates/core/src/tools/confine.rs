//! Замок путей тулов внутри рабочей директории — общий для читающих `read` и
//! `grep` ([`CanonicalCwd::confine`]) и мутирующих `write` и `edit`
//! ([`CanonicalCwd::confine_target`], [`resolve_target`]).
//!
//! `ToolCtx::resolve` только склеивает путь с cwd: абсолютный путь, `..` или
//! симлинк выводят из него куда угодно. Здесь граница проверяется по существу:
//!
//! * **Канонизируются обе стороны.** И cwd, и запрошенный путь проходят через
//!   `canonicalize` (`..` и симлинки раскрыты), и результат обязан лежать
//!   внутри канонизированного cwd. `Path::starts_with` сравнивает компоненты,
//!   так что `/work-other` за `/work` не сойдёт. Канонизировать cwd
//!   обязательно: на macOS он сам бывает симлинком (`/tmp` → `/private/tmp`),
//!   и без этого любой путь внутри выглядел бы «снаружи». Симлинк внутри cwd,
//!   ведущий наружу, после канонизации оказывается снаружи и отбивается — ради
//!   этого замок и нужен.
//! * **Только обычный файл или директория.** `File::open` на FIFO или
//!   устройстве повис бы навсегда, а отмену хода блокирующий вызов не слышит.
//!
//! Порядок проверок у читающих: сначала «есть ли путь», потом «внутри ли он».
//! Поэтому несуществующий путь снаружи отвечает `read` «нет такого», а не
//! «снаружи»: модель узнаёт о существовании чужого пути, но не о его
//! содержимом, зато текст «File not found» остался прежним.
//!
//! У мутирующих порядок обратный — и это осознанное различие, см.
//! [`CanonicalCwd::confine_target`]: цель записи обычно ещё не существует,
//! поэтому граница проверяется первой, и любой путь снаружи — существующий
//! или нет — получает один явный ответ «path is outside the working
//! directory». «Нет такого» для `write` было бы бессмыслицей (он создаёт
//! файлы), а для `edit` — ложной подсказкой «создай через `write`», которая
//! упрётся в ту же границу.
//!
//! Слова — дело тулов: модуль возвращает причину отказа ([`ConfineError`],
//! [`TargetError`]), а текст «что не так и что делать» каждый читающий тул
//! пишет сам, от своего имени. Исключение — `write` и `edit`: у них отказ
//! общий ([`resolve_target`]), он живёт здесь же, рядом с обоснованием
//! порядка проверок.
//!
//! Вызовы блокирующие (`std::fs`): из async-кода — через `spawn_blocking`
//! ([`resolve_target`] делает это сам). Почему замок есть и когда его
//! пересматривать — `docs/decisions.md`.

use std::fs::Metadata;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::ToolCtx;

/// Канонизированная рабочая директория — то, о что проверяются пути.
#[derive(Debug, Clone)]
pub(crate) struct CanonicalCwd(PathBuf);

impl CanonicalCwd {
    /// Канонизировать `cwd`. Ошибка — cwd нет или он недоступен.
    pub(crate) fn new(cwd: &Path) -> io::Result<Self> {
        cwd.canonicalize().map(Self)
    }

    /// Канонизированный путь cwd.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Канонизированный путь cwd во владение.
    pub(crate) fn into_path(self) -> PathBuf {
        self.0
    }

    /// Пропустить `path` через замок: канонизировать, проверить, что он внутри
    /// cwd, и что это обычный файл или директория. Относительный `path` — от
    /// cwd, абсолютный — как есть.
    pub(crate) fn confine(&self, path: &Path) -> Result<Confined, ConfineError> {
        // Склеиваем с cwd сами: `canonicalize` резолвил бы относительный путь
        // от cwd процесса, а не агента.
        let path = match self.0.join(path).canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ConfineError::NotFound);
            }
            Err(error) => return Err(ConfineError::Unresolvable(error)),
        };
        if !path.starts_with(&self.0) {
            return Err(ConfineError::Outside);
        }
        let metadata = std::fs::metadata(&path).map_err(ConfineError::Metadata)?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(ConfineError::NotRegular);
        }
        Ok(Confined { path, metadata })
    }

    /// Пропустить через замок цель мутирующего тула (`write`, `edit`) — путь,
    /// которого может ещё не быть. Относительный `path` — от cwd, абсолютный —
    /// как есть.
    ///
    /// `canonicalize` целиком тут не годится: он требует существования всего
    /// пути. Поэтому путь проходится покомпонентно от корня:
    ///
    /// * существующая часть резолвится по диску: обычный компонент берётся
    ///   как есть (родитель уже канонический), симлинк — через `canonicalize`
    ///   к своей цели, `..` — к родителю уже канонического пути, то есть так
    ///   же, как его понимает ядро ОС;
    /// * с первого несуществующего компонента остаток дописывается простыми
    ///   именами: его создаст `write` настоящими директориями, симлинков в нём
    ///   нет по построению. `..` после такого компонента снимает его, и
    ///   проверка диска возобновляется — `new/../link` не проскочит мимо
    ///   симлинка `link`;
    /// * висячий симлинк — отказ: запись через него создала бы файл там, куда
    ///   он указывает, а это и есть обход границы (`notes.md -> ~/.zshrc`, где
    ///   `.zshrc` ещё нет).
    ///
    /// Итог обязан лежать внутри cwd, и писать нужно именно по нему
    /// ([`Target::path`]), а не по исходному пути: тогда между проверкой и
    /// записью нет ни `..`, ни симлинков (гонку с параллельной подменой диска
    /// это не закрывает — она вне модели угроз замка).
    ///
    /// Порядок проверок обратный [`Self::confine`]: сначала «внутри ли»,
    /// потом «есть ли». Путь снаружи — [`TargetError::Outside`] независимо от
    /// того, существует ли он (ошибка диска на пути снаружи — тоже `Outside`):
    /// мутирующему тулу важна понятность отказа, и заодно модель не узнаёт,
    /// что существует снаружи.
    pub(crate) fn confine_target(&self, path: &Path) -> Result<Target, TargetError> {
        let mut resolved = PathBuf::new();
        // Сколько хвостовых компонентов `resolved` на диске ещё нет.
        let mut missing = 0usize;
        for component in self.0.join(path).components() {
            match component {
                Component::Prefix(_) | Component::RootDir => resolved.push(component),
                Component::CurDir => {}
                Component::ParentDir => {
                    // `resolved` без симлинков: лексический родитель и есть
                    // настоящий. У корня `pop` ничего не делает — `/..` это `/`.
                    resolved.pop();
                    missing = missing.saturating_sub(1);
                }
                Component::Normal(name) => {
                    let candidate = resolved.join(name);
                    if missing > 0 {
                        missing += 1;
                        resolved = candidate;
                    } else if let Some(existing) = self.existing_component(&candidate)? {
                        resolved = existing;
                    } else {
                        missing = 1;
                        resolved = candidate;
                    }
                }
            }
        }

        if !resolved.starts_with(&self.0) {
            return Err(TargetError::Outside);
        }
        if missing > 0 {
            return Ok(Target {
                path: resolved,
                metadata: None,
            });
        }
        let metadata = std::fs::metadata(&resolved).map_err(TargetError::Unresolvable)?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(TargetError::NotRegular);
        }
        Ok(Target {
            path: resolved,
            metadata: Some(metadata),
        })
    }

    /// Один компонент цели, родитель которого уже резолвлен: `Some` — он есть
    /// на диске (симлинк раскрыт до своей цели), `None` — его нет, или
    /// предыдущий компонент оказался файлом: дальше путь создаётся, и
    /// создание само скажет, если не выйдет.
    fn existing_component(&self, candidate: &Path) -> Result<Option<PathBuf>, TargetError> {
        let error = match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if !metadata.file_type().is_symlink() => {
                return Ok(Some(candidate.to_path_buf()));
            }
            Ok(_) => match candidate.canonicalize() {
                Ok(target) => return Ok(Some(target)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    TargetError::DanglingSymlink
                }
                Err(error) => TargetError::Unresolvable(error),
            },
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(None);
            }
            Err(error) => TargetError::Unresolvable(error),
        };
        Err(self.outside_or(candidate, error))
    }

    /// Отказ по пути снаружи — всегда [`TargetError::Outside`], какой бы ни
    /// была ошибка диска: граница важнее подробностей о чужих файлах.
    fn outside_or(&self, candidate: &Path, error: TargetError) -> TargetError {
        if candidate.starts_with(&self.0) {
            error
        } else {
            TargetError::Outside
        }
    }
}

/// Цель мутирующего тула, прошедшая замок.
#[derive(Debug)]
pub(crate) struct Target {
    /// Куда писать: внутри канонизированного cwd, существующая часть пути
    /// каноническая, недостающая — простыми именами.
    pub(crate) path: PathBuf,
    /// Метаданные цели, если она уже есть (обычный файл или директория);
    /// `None` — файла нет, его создаст `write`.
    pub(crate) metadata: Option<Metadata>,
}

/// Почему цель мутирующего тула не прошла замок.
#[derive(Debug)]
pub(crate) enum TargetError {
    /// Резолвленный путь вне cwd: абсолютный путь, `..` или симлинк наружу —
    /// существует цель или нет.
    Outside,
    /// Компонент пути — симлинк, цели которого нет.
    DanglingSymlink,
    /// Цель — не обычный файл и не директория: FIFO, сокет, устройство.
    NotRegular,
    /// Диск ответил иначе: нет прав на компонент и т. п.
    Unresolvable(io::Error),
}

/// Пропустить цель `write`/`edit` через замок — вне потока рантайма — и при
/// отказе сразу вернуть текст для модели от имени `tool`.
///
/// Текст «outside» сформулирован как у `read`, но показывается для любого
/// пути снаружи, а не только для существующего (почему — в
/// [`CanonicalCwd::confine_target`]).
pub(crate) async fn resolve_target(
    tool: &'static str,
    ctx: &ToolCtx,
    requested: &str,
) -> Result<Target, String> {
    let cwd = ctx.cwd.clone();
    let display = ctx.resolve(requested);
    let owned = requested.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        let canonical_cwd = CanonicalCwd::new(&cwd).map_err(|error| {
            format!(
                "Cannot write {}: the working directory {} is not accessible: {error}",
                display.display(),
                cwd.display()
            )
        })?;
        canonical_cwd
            .confine_target(Path::new(&owned))
            .map_err(|error| target_refusal(tool, &owned, &display, &cwd, error))
    })
    .await;
    joined.unwrap_or_else(|error| Err(format!("Cannot write {requested}: {error}")))
}

/// Текст отказа мутирующего тула. `display` — путь, как его видит модель
/// (склеенный с cwd), `cwd` — как задан, без канонизации.
fn target_refusal(
    tool: &str,
    requested: &str,
    display: &Path,
    cwd: &Path,
    error: TargetError,
) -> String {
    let display = display.display();
    match error {
        TargetError::Outside => format!(
            "Path {requested} is outside the working directory {} (after resolving `..` and \
             symlinks): `{tool}` only changes files inside the working directory, and retrying \
             this path will fail the same way. Pass a path within the working directory; \
             relative paths resolve against it.",
            cwd.display()
        ),
        TargetError::DanglingSymlink => format!(
            "{display} goes through a symlink whose target does not exist: `{tool}` does not \
             write through dangling symlinks. Use the real path inside the working directory."
        ),
        TargetError::NotRegular => format!(
            "{display} is not a regular file (a FIFO, socket or device): `{tool}` cannot write it."
        ),
        TargetError::Unresolvable(error) => format!("Cannot write {display}: {error}"),
    }
}

/// Путь, прошедший замок.
#[derive(Debug)]
pub(crate) struct Confined {
    /// Канонический путь внутри cwd.
    pub(crate) path: PathBuf,
    /// Метаданные канонического пути: обычный файл или директория.
    pub(crate) metadata: Metadata,
}

/// Почему путь не прошёл замок.
#[derive(Debug)]
pub(crate) enum ConfineError {
    /// Пути нет (или это висячий симлинк). Отдельно от прочих причин: модель
    /// должна понять «файла нет», а не «доступ запрещён».
    NotFound,
    /// Канонизация упала иначе: нет прав на компонент, компонент — не
    /// директория и т. п.
    Unresolvable(io::Error),
    /// Канонический путь вне cwd: абсолютный путь, `..` или симлинк наружу.
    Outside,
    /// `metadata` канонического пути не снялась.
    Metadata(io::Error),
    /// Не обычный файл и не директория: FIFO, сокет, устройство.
    NotRegular,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// `parent/work` — cwd; `parent/secret.txt` и `parent/work2/` — снаружи.
    fn sandbox() -> (TempDir, CanonicalCwd) {
        let parent = TempDir::new().unwrap();
        write(parent.path(), "secret.txt", "secret");
        write(parent.path(), "work2/secret.txt", "secret");
        write(parent.path(), "work/a.txt", "a");
        std::fs::create_dir_all(parent.path().join("work/sub")).unwrap();
        let cwd = CanonicalCwd::new(&parent.path().join("work")).unwrap();
        (parent, cwd)
    }

    #[test]
    fn paths_inside_the_cwd_are_canonical_files_or_directories() {
        let (_parent, cwd) = sandbox();
        let file = cwd.path().join("a.txt");
        for path in [
            PathBuf::from("a.txt"),
            PathBuf::from("sub/../a.txt"),
            PathBuf::from("./a.txt"),
            file.clone(),
        ] {
            let confined = cwd.confine(&path).unwrap();
            assert_eq!(confined.path, file, "{}", path.display());
            assert!(confined.metadata.is_file(), "{}", path.display());
        }
        let confined = cwd.confine(Path::new("sub")).unwrap();
        assert!(confined.metadata.is_dir());
        // Сам cwd — тоже внутри.
        assert_eq!(cwd.confine(Path::new(".")).unwrap().path, cwd.path());
    }

    #[test]
    fn paths_leading_outside_are_rejected() {
        let (parent, cwd) = sandbox();
        for path in [
            PathBuf::from(".."),
            PathBuf::from("../secret.txt"),
            PathBuf::from("sub/../../secret.txt"),
            // Соседняя директория с общим префиксом имени — тоже снаружи.
            PathBuf::from("../work2/secret.txt"),
            parent.path().join("secret.txt"),
            PathBuf::from("/"),
        ] {
            let result = cwd.confine(&path);
            assert!(
                matches!(result, Err(ConfineError::Outside)),
                "{}: {result:?}",
                path.display()
            );
        }
    }

    #[test]
    fn missing_paths_are_not_found_rather_than_outside() {
        let (_parent, cwd) = sandbox();
        for path in ["nope.txt", "sub/nope.txt"] {
            let result = cwd.confine(Path::new(path));
            assert!(
                matches!(result, Err(ConfineError::NotFound)),
                "{path}: {result:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_resolved_before_the_check() {
        use std::os::unix::fs::symlink;
        let (parent, cwd) = sandbox();
        let work = cwd.path();
        symlink(parent.path().join("secret.txt"), work.join("out_file")).unwrap();
        symlink(parent.path().join("work2"), work.join("out_dir")).unwrap();
        symlink(work.join("a.txt"), work.join("in_file")).unwrap();
        symlink(work.join("gone.txt"), work.join("dangling")).unwrap();

        for path in ["out_file", "out_dir", "out_dir/secret.txt"] {
            let result = cwd.confine(Path::new(path));
            assert!(
                matches!(result, Err(ConfineError::Outside)),
                "{path}: {result:?}"
            );
        }
        // Симлинк внутри cwd, ведущий внутрь, — законен; путь уже канонический.
        let confined = cwd.confine(Path::new("in_file")).unwrap();
        assert_eq!(confined.path, work.join("a.txt"));
        // Висячий симлинк — «нет такого», как и раньше у `read`.
        assert!(matches!(
            cwd.confine(Path::new("dangling")),
            Err(ConfineError::NotFound)
        ));
    }

    /// Грабля macOS (`/tmp` → `/private/tmp`), воспроизведённая на любом unix:
    /// cwd, заданный через симлинк, не делает свои же файлы «внешними».
    #[cfg(unix)]
    #[test]
    fn cwd_behind_a_symlink_still_contains_its_files() {
        let (parent, _cwd) = sandbox();
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(parent.path().join("work"), &alias).unwrap();

        let cwd = CanonicalCwd::new(&alias).unwrap();
        for path in [PathBuf::from("a.txt"), alias.join("a.txt")] {
            assert!(cwd.confine(&path).is_ok(), "{}", path.display());
        }
        assert!(matches!(
            cwd.confine(&alias.join("../secret.txt")),
            Err(ConfineError::Outside)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_not_a_regular_file() {
        let (_parent, cwd) = sandbox();
        let status = std::process::Command::new("mkfifo")
            .arg(cwd.path().join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            cwd.confine(Path::new("pipe")),
            Err(ConfineError::NotRegular)
        ));
    }

    #[test]
    fn missing_cwd_is_an_error() {
        let parent = TempDir::new().unwrap();
        assert!(CanonicalCwd::new(&parent.path().join("nope")).is_err());
    }

    // --- Цели мутирующих тулов (`confine_target`) ---

    fn target(cwd: &CanonicalCwd, path: impl AsRef<Path>) -> Result<Target, TargetError> {
        cwd.confine_target(path.as_ref())
    }

    fn assert_outside(cwd: &CanonicalCwd, path: impl AsRef<Path>) {
        let path = path.as_ref();
        let result = target(cwd, path);
        assert!(
            matches!(result, Err(TargetError::Outside)),
            "{}: {result:?}",
            path.display()
        );
    }

    /// Новый файл внутри cwd (в том числе в несуществующих подкаталогах) —
    /// законная цель без метаданных; существующий — с ними.
    #[test]
    fn targets_inside_the_cwd_may_or_may_not_exist() {
        let (_parent, cwd) = sandbox();
        let work = cwd.path();

        let new = target(&cwd, "deep/nested/new.txt").unwrap();
        assert_eq!(new.path, work.join("deep/nested/new.txt"));
        assert!(new.metadata.is_none());

        for path in [
            PathBuf::from("a.txt"),
            PathBuf::from("./sub/../a.txt"),
            work.join("a.txt"),
        ] {
            let existing = target(&cwd, &path).unwrap();
            assert_eq!(existing.path, work.join("a.txt"), "{}", path.display());
            assert!(existing.metadata.unwrap().is_file(), "{}", path.display());
        }

        // `..` после несуществующего компонента снимает его, а не создаёт.
        assert_eq!(
            target(&cwd, "ghost/../b.txt").unwrap().path,
            work.join("b.txt")
        );
        // Сам cwd и поддиректория — существующие директории (`write` откажет
        // текстом «is a directory»).
        assert!(target(&cwd, ".").unwrap().metadata.unwrap().is_dir());
        assert!(target(&cwd, "sub").unwrap().metadata.unwrap().is_dir());
        // Компонент-файл посреди пути — не отказ замка: путь внутри, а
        // создание каталога скажет само, что не выйдет.
        let under_file = target(&cwd, "a.txt/child.txt").unwrap();
        assert_eq!(under_file.path, work.join("a.txt/child.txt"));
        assert!(under_file.metadata.is_none());
    }

    /// Снаружи — всегда `Outside`, существует путь или нет: у мутирующих
    /// тулов граница проверяется раньше существования.
    #[test]
    fn targets_outside_are_rejected_whether_or_not_they_exist() {
        let (parent, cwd) = sandbox();
        for path in [
            PathBuf::from("../escape.txt"),
            PathBuf::from("../secret.txt"),
            PathBuf::from("sub/../../secret.txt"),
            PathBuf::from("ghost/../../escape.txt"),
            PathBuf::from("../work2/new.txt"),
            PathBuf::from("../nope/deeper/new.txt"),
            parent.path().join("secret.txt"),
            parent.path().join("escape.txt"),
            PathBuf::from("/"),
            PathBuf::from(".."),
        ] {
            assert_outside(&cwd, &path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn target_symlinks_are_resolved_before_the_check() {
        use std::os::unix::fs::symlink;
        let (parent, cwd) = sandbox();
        let work = cwd.path();
        symlink(parent.path().join("secret.txt"), work.join("out_file")).unwrap();
        symlink(parent.path().join("work2"), work.join("out_dir")).unwrap();
        symlink(work.join("a.txt"), work.join("in_file")).unwrap();
        symlink(work.join("sub"), work.join("in_dir")).unwrap();
        // Висячие: внутрь и наружу (`~/.zshrc`, которого ещё нет).
        symlink(work.join("gone.txt"), work.join("dangling_in")).unwrap();
        symlink(parent.path().join("gone.txt"), work.join("dangling_out")).unwrap();

        for path in [
            "out_file",
            "out_dir",
            "out_dir/secret.txt",
            "out_dir/new.txt",
            "out_dir/new/deeper.txt",
            // `..` после несуществующего компонента возвращает проверку диска.
            "ghost/../out_file",
            // `..` после симлинка — от его цели, как у ядра ОС.
            "in_dir/../../secret.txt",
        ] {
            assert_outside(&cwd, path);
        }

        // Симлинк внутри, ведущий внутрь, — законен; путь — уже цель.
        let file = target(&cwd, "in_file").unwrap();
        assert_eq!(file.path, work.join("a.txt"));
        assert!(file.metadata.unwrap().is_file());
        let through_dir = target(&cwd, "in_dir/new.txt").unwrap();
        assert_eq!(through_dir.path, work.join("sub/new.txt"));
        assert!(through_dir.metadata.is_none());

        for path in ["dangling_in", "dangling_out"] {
            let result = target(&cwd, path);
            assert!(
                matches!(result, Err(TargetError::DanglingSymlink)),
                "{path}: {result:?}"
            );
        }
    }

    /// macOS: cwd через симлинк (`/tmp` → `/private/tmp`) — его файлы всё
    /// равно внутри, и абсолютный путь через симлинк тоже.
    #[cfg(unix)]
    #[test]
    fn target_cwd_behind_a_symlink_still_contains_its_files() {
        let (parent, _cwd) = sandbox();
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(parent.path().join("work"), &alias).unwrap();

        let cwd = CanonicalCwd::new(&alias).unwrap();
        for path in [
            PathBuf::from("new.txt"),
            alias.join("new.txt"),
            alias.join("a.txt"),
        ] {
            assert!(target(&cwd, &path).is_ok(), "{}", path.display());
        }
        assert_outside(&cwd, alias.join("../escape.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn target_fifo_is_not_a_regular_file() {
        let (_parent, cwd) = sandbox();
        let status = std::process::Command::new("mkfifo")
            .arg(cwd.path().join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(target(&cwd, "pipe"), Err(TargetError::NotRegular)));
    }

    /// Текст отказа: явное «outside» с именем тула — и для несуществующего
    /// пути снаружи тоже (у `read` там было бы «File not found»).
    #[tokio::test]
    async fn refusal_names_the_boundary_and_the_tool() {
        let (_parent, cwd) = sandbox();
        let ctx = ToolCtx::new(cwd.path());
        for requested in ["../escape.txt", "../secret.txt"] {
            let message = resolve_target("edit", &ctx, requested).await.unwrap_err();
            assert!(
                message.starts_with(&format!(
                    "Path {requested} is outside the working directory"
                )),
                "{message}"
            );
            assert!(
                message.contains("`edit` only changes files inside the working directory"),
                "{message}"
            );
            assert!(!message.contains("not found"), "{message}");
        }

        let missing_cwd = ToolCtx::new(cwd.path().join("gone"));
        let message = resolve_target("write", &missing_cwd, "a.txt")
            .await
            .unwrap_err();
        assert!(message.contains("is not accessible"), "{message}");
    }
}
