' corpus-source: generated
' corpus-flagged: no
' corpus-prints: done
Sub Main()
    Application.ScreenUpdating = False
    Application.Calculation = xlCalculationManual
    Range("A1").Value = 1
    Application.Calculation = xlCalculationAutomatic
    Application.ScreenUpdating = True
    Application.StatusBar = "Working"
    Application.StatusBar = False
    Debug.Print "done"
End Sub
